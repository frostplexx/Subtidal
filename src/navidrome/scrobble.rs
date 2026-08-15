// Scrobble reporting: best-effort fan-out from the scrobble handler to
// configurable backends (Last.fm, ListenBrainz). A failing reporter only
// logs; it never fails the client request. The reporter registry is built
// once at startup from settings.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use crate::navidrome::auth::md5_hex;
use crate::settings::Settings;

// A scrobble payload: one track played to completion. Artist is the
// primary artist (plain name); album and duration feed both backends.
#[derive(Clone, Debug)]
pub struct ScrobbleSong {
    pub track_id: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: u32,
}

// A scrobble backend. `timestamp_ms` is epoch milliseconds; each backend
// converts to its own unit (unix seconds). BoxFuture keeps the trait
// dyn-compatible (async fn in traits is not).
pub trait PlayReporter: Send + Sync {
    fn name(&self) -> &'static str;
    fn report<'a>(
        &'a self,
        song: &'a ScrobbleSong,
        timestamp_ms: i64,
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>>;
}

const KEYRING_SERVICE: &str = "Subtidal";
const LASTFM_KEYRING_USER: &str = "lastfm-session-key";
const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const LASTFM_AUTH_URL: &str = "https://www.last.fm/api/auth/";
const LISTENBRAINZ_API_BASE: &str = "https://api.listenbrainz.org";

// The configured reporter list, swappable by tests. The Arc lets
// report_song clone the list cheaply and release the lock before the
// network calls.
type ReporterRegistry = Mutex<Arc<Vec<Box<dyn PlayReporter>>>>;

fn registry() -> &'static ReporterRegistry {
    static REGISTRY: OnceLock<ReporterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Arc::new(Vec::new())))
}

// Build the reporter list from settings. The Last.fm reporter needs a
// session key from the OS keychain; a missing key logs a warning and
// skips that reporter. Tests replace the registry wholesale.
pub fn init(settings: &Settings) {
    let mut reporters: Vec<Box<dyn PlayReporter>> = Vec::new();
    if let Some(cfg) = &settings.lastfm {
        match lastfm_session_key() {
            Ok(Some(sk)) => reporters.push(Box::new(LastFmReporter::new(
                cfg.api_key.clone(),
                cfg.api_secret.clone(),
                sk,
                LASTFM_API_URL,
            ))),
            Ok(None) => tracing::warn!(
                "lastfm: no session key in keychain; run `subtidal --lastfm-auth` once"
            ),
            Err(e) => tracing::warn!("lastfm: session key unavailable: {e}"),
        }
    }
    if let Some(cfg) = &settings.listenbrainz {
        reporters.push(Box::new(ListenBrainzReporter::new(
            cfg.token.clone(),
            LISTENBRAINZ_API_BASE,
        )));
    }
    *registry().lock().unwrap() = Arc::new(reporters);
}

// True when at least one scrobble backend is configured. Drives
// getUser's scrobblingEnabled flag.
pub fn enabled() -> bool {
    !registry().lock().unwrap().is_empty()
}

// Report a completed track to every configured backend. Errors log at
// warn; the client request never fails because of a reporter.
pub async fn report_song(song: &ScrobbleSong, timestamp_ms: i64) {
    // Arc clone releases the registry lock before the network calls.
    let reporters = registry().lock().unwrap().clone();
    for reporter in reporters.iter() {
        if let Err(e) = reporter.report(song, timestamp_ms).await {
            tracing::warn!(
                "scrobble failed ({}) for {}: {e}",
                reporter.name(),
                song.track_id
            );
        }
    }
}

// Map a Tidal track JSON to a scrobble payload. The artist is the first
// track artist, plain name (no "feat." chain). Returns None without an id.
pub fn scrobble_song_from_track(v: &Value) -> Option<ScrobbleSong> {
    let track_id = v["id"].as_u64()?;
    let title = v["title"].as_str()?.to_string();
    let artist = v["artists"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|a| a["name"].as_str())
        .unwrap_or("Unknown Artist")
        .to_string();
    let album = v["album"]["title"].as_str().map(String::from);
    let duration = v["duration"].as_u64().unwrap_or(0) as u32;
    Some(ScrobbleSong {
        track_id,
        title,
        artist,
        album,
        duration,
    })
}

// ---------------------------------------------------------------------------
// Last.fm
// ---------------------------------------------------------------------------

pub struct LastFmReporter {
    api_key: String,
    api_secret: String,
    session_key: String,
    api_url: String,
    http: reqwest::Client,
}

impl LastFmReporter {
    pub fn new(api_key: String, api_secret: String, session_key: String, api_url: &str) -> Self {
        Self {
            api_key,
            api_secret,
            session_key,
            api_url: api_url.to_string(),
            http: reqwest::Client::new(),
        }
    }

    // MD5 API signature: sort params by key, concatenate keyvalue pairs,
    // append the secret. The "format" param is excluded.
    fn sign(&self, params: &BTreeMap<&str, String>) -> String {
        let mut sig_input = String::new();
        for (k, v) in params {
            if *k == "format" {
                continue;
            }
            sig_input.push_str(k);
            sig_input.push_str(v);
        }
        sig_input.push_str(&self.api_secret);
        md5_hex(sig_input)
    }

    async fn post(&self, params: BTreeMap<&str, String>) -> Result<Value, String> {
        let body = self
            .http
            .post(&self.api_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let json: Value = body
            .json()
            .await
            .map_err(|e| format!("response parse failed: {e}"))?;
        if let Some(code) = json.get("error").and_then(|e| e.as_u64()) {
            let msg = json
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(format!("error {code}: {msg}"));
        }
        Ok(json)
    }
}

impl PlayReporter for LastFmReporter {
    fn name(&self) -> &'static str {
        "lastfm"
    }

    fn report<'a>(
        &'a self,
        song: &'a ScrobbleSong,
        timestamp_ms: i64,
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut params: BTreeMap<&str, String> = BTreeMap::new();
            params.insert("method", "track.scrobble".into());
            params.insert("api_key", self.api_key.clone());
            params.insert("sk", self.session_key.clone());
            params.insert("artist", song.artist.clone());
            params.insert("track", song.title.clone());
            params.insert("timestamp", (timestamp_ms / 1000).to_string());
            params.insert("duration", song.duration.to_string());
            if let Some(album) = &song.album {
                params.insert("album", album.clone());
            }
            let sig = self.sign(&params);
            params.insert("api_sig", sig);
            params.insert("format", "json".into());
            match self.post(params).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    tracing::warn!("lastfm: scrobble error: {e}");
                    Ok(())
                }
            }
        })
    }
}

// The Last.fm session key from the OS keychain, if present.
pub fn lastfm_session_key() -> Result<Option<String>, String> {
    match keyring::Entry::new(KEYRING_SERVICE, LASTFM_KEYRING_USER).and_then(|e| e.get_password()) {
        Ok(sk) => Ok(Some(sk)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring get failed: {e}")),
    }
}

// One-time Last.fm auth, used by `subtidal --lastfm-auth`:
// getToken -> print the authorize URL -> wait for the user -> getSession
// -> store the session key in the keychain.
pub async fn lastfm_auth_flow(api_key: &str, api_secret: &str) -> Result<(), String> {
    let http = reqwest::Client::new();
    let mut params: BTreeMap<&str, String> = BTreeMap::new();
    params.insert("method", "auth.getToken".into());
    params.insert("api_key", api_key.into());
    let sig = {
        let mut sig_input = String::new();
        for (k, v) in &params {
            sig_input.push_str(k);
            sig_input.push_str(v);
        }
        sig_input.push_str(api_secret);
        md5_hex(sig_input)
    };
    params.insert("api_sig", sig);
    params.insert("format", "json".into());
    let body: Value = http
        .post(LASTFM_API_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("auth.getToken request failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("auth.getToken parse failed: {e}"))?;
    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "auth.getToken: missing token".to_string())?;

    println!("Open this URL, authorize the app, then press Enter:");
    println!("{LASTFM_AUTH_URL}?api_key={api_key}&token={token}");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("stdin read failed: {e}"))?;

    let mut params: BTreeMap<&str, String> = BTreeMap::new();
    params.insert("method", "auth.getSession".into());
    params.insert("api_key", api_key.into());
    params.insert("token", token.into());
    let sig = {
        let mut sig_input = String::new();
        for (k, v) in &params {
            sig_input.push_str(k);
            sig_input.push_str(v);
        }
        sig_input.push_str(api_secret);
        md5_hex(sig_input)
    };
    params.insert("api_sig", sig);
    params.insert("format", "json".into());
    let body: Value = http
        .post(LASTFM_API_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("auth.getSession request failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("auth.getSession parse failed: {e}"))?;
    let session = body
        .get("session")
        .ok_or_else(|| "auth.getSession: missing session".to_string())?;
    let key = session
        .get("key")
        .and_then(|k| k.as_str())
        .ok_or_else(|| "auth.getSession: missing key".to_string())?;
    let name = session
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| "auth.getSession: missing name".to_string())?;

    keyring::Entry::new(KEYRING_SERVICE, LASTFM_KEYRING_USER)
        .and_then(|e| e.set_password(key))
        .map_err(|e| format!("keyring set failed: {e}"))?;
    println!("Last.fm authenticated as {name}; session key stored in the keychain.");
    Ok(())
}

// ---------------------------------------------------------------------------
// ListenBrainz
// ---------------------------------------------------------------------------

pub struct ListenBrainzReporter {
    token: String,
    api_base: String,
    http: reqwest::Client,
}

impl ListenBrainzReporter {
    pub fn new(token: String, api_base: &str) -> Self {
        Self {
            token,
            api_base: api_base.to_string(),
            http: reqwest::Client::new(),
        }
    }
}

impl PlayReporter for ListenBrainzReporter {
    fn name(&self) -> &'static str {
        "listenbrainz"
    }

    fn report<'a>(
        &'a self,
        song: &'a ScrobbleSong,
        timestamp_ms: i64,
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut additional_info = serde_json::json!({
                "media_player": "Subtidal",
                "submission_client": "Subtidal",
                "music_service": "tidal.com",
            });
            if song.duration > 0 {
                additional_info["duration"] = serde_json::json!(song.duration);
            }
            additional_info["origin_url"] =
                serde_json::json!(format!("https://listen.tidal.com/track/{}", song.track_id));

            let mut metadata = serde_json::json!({
                "artist_name": song.artist,
                "track_name": song.title,
                "additional_info": additional_info,
            });
            if let Some(album) = &song.album {
                metadata["release_name"] = serde_json::json!(album);
            }

            let body = serde_json::json!({
                "listen_type": "single",
                "payload": [{
                    "listened_at": timestamp_ms / 1000,
                    "track_metadata": metadata,
                }],
            });

            let resp = self
                .http
                .post(format!("{}/1/submit-listens", self.api_base))
                .header("Authorization", format!("Token {}", self.token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("request failed: {e}"))?;
            match resp.status().as_u16() {
                200..=299 => Ok(()),
                401 => Err("unauthorized: check the ListenBrainz token".into()),
                429 => Err("rate limited".into()),
                status => {
                    let text = resp.text().await.unwrap_or_default();
                    Err(format!("HTTP {status}: {text}"))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use warp::Filter;

    // The registry is process-global; a lock serializes the tests that
    // replace it, mirroring the play_state module's pattern.
    fn registry_lock() -> std::sync::MutexGuard<'static, ()> {
        static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn scrobble_song() -> ScrobbleSong {
        ScrobbleSong {
            track_id: 463900374,
            title: "Down Bad".into(),
            artist: "Taylor Swift".into(),
            album: Some("THE TORTURED POETS DEPARTMENT".into()),
            duration: 201,
        }
    }

    #[test]
    fn maps_track_json_to_song() {
        let song = scrobble_song_from_track(&json!({
            "id": 463900374,
            "title": "Down Bad",
            "artists": [{"id": 3557299, "name": "Taylor Swift"}],
            "album": {"id": 357676034, "title": "THE TORTURED POETS DEPARTMENT"},
            "duration": 201
        }))
        .unwrap();
        assert_eq!(song.artist, "Taylor Swift");
        assert_eq!(song.album.as_deref(), Some("THE TORTURED POETS DEPARTMENT"));
        assert_eq!(song.duration, 201);
    }

    #[test]
    fn unknown_track_json_maps_to_none() {
        assert!(scrobble_song_from_track(&json!({ "title": "no id" })).is_none());
    }

    #[test]
    fn lastfm_sign_matches_sorted_concat() {
        // Params sort by key: api_key, artist, method. The signature is
        // md5(api_key<v>artist<v>method<v> + api_secret), excluding format.
        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("api_key", "key123".into());
        params.insert("artist", "Taylor Swift".into());
        params.insert("method", "track.scrobble".into());
        params.insert("format", "json".into());
        let reporter = LastFmReporter::new(
            "key123".into(),
            "secret".into(),
            "sk".into(),
            LASTFM_API_URL,
        );
        let input = "api_keykey123artistTaylor Swiftmethodtrack.scrobble".to_string() + "secret";
        assert_eq!(reporter.sign(&params), md5_hex(&input));
    }

    // A recording reporter for fan-out tests.
    struct RecordingReporter {
        calls: Arc<Mutex<Vec<(u64, i64)>>>,
        fail: bool,
    }

    impl PlayReporter for RecordingReporter {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn report<'a>(
            &'a self,
            song: &'a ScrobbleSong,
            timestamp_ms: i64,
        ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((song.track_id, timestamp_ms));
                if self.fail {
                    Err("boom".into())
                } else {
                    Ok(())
                }
            })
        }
    }

    // The registry lock intentionally covers the report_song await: the
    // test asserts on the recorded calls afterwards.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn report_song_fans_out_to_all_reporters() {
        let _g = registry_lock();
        let calls = Arc::new(Mutex::new(Vec::new()));
        *registry().lock().unwrap() = Arc::new(vec![
            Box::new(RecordingReporter {
                calls: calls.clone(),
                fail: false,
            }),
            Box::new(RecordingReporter {
                calls: calls.clone(),
                fail: true,
            }),
        ]);
        report_song(&scrobble_song(), 1_786_116_785_370).await;
        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            calls.len(),
            2,
            "a failing reporter must not stop the fan-out"
        );
        assert!(calls.iter().all(|(id, _)| *id == 463900374));
        assert!(calls.iter().all(|(_, t)| *t == 1_786_116_785_370));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn report_song_with_no_reporters_is_a_noop() {
        let _g = registry_lock();
        *registry().lock().unwrap() = Arc::new(Vec::new());
        report_song(&scrobble_song(), 0).await; // must not panic
    }

    // Spin up a local warp server on an ephemeral port. Every request is
    // captured (raw body) and answered 200 with "ok".
    async fn mock_server() -> (String, Arc<Mutex<Vec<String>>>) {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        let route = warp::body::bytes().map(move |body: bytes::Bytes| {
            cap.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&body).to_string());
            warp::reply::with_status("ok".to_string(), warp::http::StatusCode::OK)
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        // Bind eagerly so the socket is listening before the client fires;
        // the spawned task only accepts. It lives for the test and is
        // dropped with the test runtime.
        let bound = warp::serve(route).bind(addr).await;
        tokio::spawn(bound.run());
        (format!("http://{addr}"), captured)
    }

    #[tokio::test]
    async fn lastfm_report_posts_signed_scrobble() {
        let (base, captured) = mock_server().await;
        let reporter = LastFmReporter::new(
            "key".into(),
            "secret".into(),
            "sk123".into(),
            &format!("{base}/2.0/"),
        );
        reporter
            .report(&scrobble_song(), 1_786_116_785_370)
            .await
            .unwrap();
        let body = captured.lock().unwrap().join("");
        assert!(body.contains("method=track.scrobble"), "{body}");
        assert!(body.contains("artist=Taylor+Swift"), "{body}");
        assert!(body.contains("track=Down+Bad"), "{body}");
        assert!(body.contains("timestamp=1786116785"), "{body}");
        assert!(body.contains("duration=201"), "{body}");
        assert!(body.contains("sk=sk123"), "{body}");
        assert!(body.contains("api_sig="), "{body}");
        // 'format' is excluded from the signature but still sent.
        assert!(body.contains("format=json"), "{body}");
    }

    #[tokio::test]
    async fn lastfm_report_survives_api_error() {
        let (base, _captured) = mock_server().await;
        // The mock answers "ok" for any path; that is not JSON, so the
        // reporter logs the parse failure and still returns Ok.
        let reporter = LastFmReporter::new(
            "key".into(),
            "secret".into(),
            "sk".into(),
            &format!("{base}/nope"),
        );
        assert!(reporter.report(&scrobble_song(), 0).await.is_ok());
    }

    #[tokio::test]
    async fn listenbrainz_report_posts_payload() {
        let (base, captured) = mock_server().await;
        let reporter = ListenBrainzReporter::new("tok123".into(), &base);
        reporter
            .report(&scrobble_song(), 1_786_116_785_370)
            .await
            .unwrap();
        let body = captured.lock().unwrap().join("");
        let json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["listen_type"], "single");
        assert_eq!(json["payload"][0]["listened_at"], 1_786_116_785);
        assert_eq!(
            json["payload"][0]["track_metadata"]["artist_name"],
            "Taylor Swift"
        );
        assert_eq!(
            json["payload"][0]["track_metadata"]["track_name"],
            "Down Bad"
        );
        assert_eq!(
            json["payload"][0]["track_metadata"]["release_name"],
            "THE TORTURED POETS DEPARTMENT"
        );
        assert_eq!(
            json["payload"][0]["track_metadata"]["additional_info"]["duration"],
            201
        );
        assert_eq!(
            json["payload"][0]["track_metadata"]["additional_info"]["origin_url"],
            "https://listen.tidal.com/track/463900374"
        );
        assert_eq!(
            json["payload"][0]["track_metadata"]["additional_info"]["music_service"],
            "tidal.com"
        );
    }
}
