// Scrobble and now-playing reporting: best-effort fan-out from the
// scrobble, updateNowPlaying, and reportPlayback handlers to
// configurable backends (Last.fm, ListenBrainz). A failing reporter
// only logs; it never fails the client request. The reporter registry
// is built once at startup from settings.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use qrcode::QrCode;
use qrcode::render::unicode;
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
    // Now-playing notification: the song currently playing. Best-effort
    // like report; the default is a no-op.
    fn now_playing<'a>(
        &'a self,
        song: &'a ScrobbleSong,
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let _ = song;
            Ok(())
        })
    }
}

const KEYRING_SERVICE: &str = "Subtidal";
const LASTFM_KEYRING_USER: &str = "lastfm-session-key";
const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const LASTFM_AUTH_URL: &str = "https://www.last.fm/api/auth/";
const LISTENBRAINZ_API_BASE: &str = "https://api.listenbrainz.org";

// Where the fallback session key lives when the OS keyring is
// unavailable (headless Linux without a Secret Service). Override with
// SUBTIDAL_LASTFM_KEY_FILE; default is
// $XDG_STATE_HOME/subtidal/lastfm-session-key.
const LASTFM_KEY_FILE_ENV: &str = "SUBTIDAL_LASTFM_KEY_FILE";

fn lastfm_key_file() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os(LASTFM_KEY_FILE_ENV) {
        return Some(std::path::PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state"))
        })?;
    Some(base.join("subtidal").join(LASTFM_KEYRING_USER))
}

fn read_key_file_at(path: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("key file read failed ({}): {e}", path.display())),
    }
}

fn read_key_file() -> Result<Option<String>, String> {
    match lastfm_key_file() {
        Some(path) => read_key_file_at(&path),
        None => Ok(None),
    }
}

fn write_key_file(path: &std::path::Path, key: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, key)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

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
        // The reporter is always added: a missing session key only skips
        // individual reports, and a background re-authorization stores
        // a new key without rebuilding the registry.
        if lastfm_session_key().ok().flatten().is_none() {
            tracing::warn!("lastfm: not authorized; scrobbles are skipped until authorized");
        }
        reporters.push(Box::new(LastFmReporter::new(
            cfg.api_key.clone(),
            cfg.api_secret.clone(),
            LASTFM_API_URL,
        )));
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

// A track start can arrive through three signals (scrobble with
// submission=false, updateNowPlaying, reportPlayback), so the guard
// reports one now-playing update per track per cooldown. The cooldown
// still lets a repeat play of the same track refresh the notification.
const NP_COOLDOWN_MS: i64 = 120_000;

struct LastNowPlaying {
    track_id: u64,
    at_ms: i64,
}

fn np_due(track_id: u64, now: i64, last: &Option<LastNowPlaying>) -> bool {
    match last {
        None => true,
        Some(l) => l.track_id != track_id || now - l.at_ms > NP_COOLDOWN_MS,
    }
}

fn last_np() -> &'static Mutex<Option<LastNowPlaying>> {
    static LAST: OnceLock<Mutex<Option<LastNowPlaying>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

// Report the currently playing track to every backend. Best-effort:
// track metadata is fetched through the Tidal client; when it is
// unavailable the report is skipped and logged, never an error.
pub(crate) async fn report_now_playing(track_id: u64) {
    {
        let mut last = last_np().lock().unwrap();
        let now = crate::navidrome::now_playing::now_ms();
        if !np_due(track_id, now, &last) {
            return;
        }
        *last = Some(LastNowPlaying {
            track_id,
            at_ms: now,
        });
    }
    let Some(client) = crate::tidal::client_opt() else {
        tracing::warn!("now-playing id={track_id}: tidal client unavailable; skipped");
        return;
    };
    let detail = match client.track(track_id).await {
        Ok(v) => v.to_json(),
        Err(e) => {
            tracing::warn!("now-playing id={track_id}: track fetch failed: {e}");
            return;
        }
    };
    let Some(song) = scrobble_song_from_track(&detail) else {
        tracing::warn!("now-playing id={track_id}: track metadata incomplete; skipped");
        return;
    };
    report_now_playing_song(&song).await;
}

// Fan out one now-playing notification to every configured reporter.
// A failing reporter only logs; it never fails the caller.
pub(crate) async fn report_now_playing_song(song: &ScrobbleSong) {
    let reporters = registry().lock().unwrap().clone();
    for reporter in reporters.iter() {
        if let Err(e) = reporter.now_playing(song).await {
            tracing::warn!(
                "now-playing failed ({}) for {}: {e}",
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

// Re-authorization trigger: spawns the interactive flow in
// production. Tests inject a recorder.
type ReauthFn = Box<dyn Fn(&str, &str) + Send + Sync>;

pub struct LastFmReporter {
    api_key: String,
    api_secret: String,
    api_url: String,
    http: reqwest::Client,
    // Session key provider: the OS keychain in production, so a
    // background re-authorization is picked up on the next report.
    // Tests inject a fake.
    session_key: Box<dyn Fn() -> Result<Option<String>, String> + Send + Sync>,
    // Re-authorization trigger: spawns the interactive flow in
    // production. Tests inject a recorder.
    reauth: ReauthFn,
}

impl LastFmReporter {
    pub fn new(api_key: String, api_secret: String, api_url: &str) -> Self {
        Self {
            api_key,
            api_secret,
            api_url: api_url.to_string(),
            http: reqwest::Client::new(),
            session_key: Box::new(lastfm_session_key),
            reauth: Box::new(trigger_reauthorization),
        }
    }

    #[cfg(test)]
    fn new_with_fakes(
        api_key: String,
        api_secret: String,
        api_url: &str,
        session_key: Box<dyn Fn() -> Result<Option<String>, String> + Send + Sync>,
        reauth: ReauthFn,
    ) -> Self {
        Self {
            api_key,
            api_secret,
            api_url: api_url.to_string(),
            http: reqwest::Client::new(),
            session_key,
            reauth,
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

    // Last.fm API POST. Errors carry (code, message); a transport or
    // parse failure has code 0.
    async fn post(&self, params: BTreeMap<&str, String>) -> Result<Value, (u32, String)> {
        let body = self
            .http
            .post(&self.api_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| (0, format!("request failed: {e}")))?;
        let json: Value = body
            .json()
            .await
            .map_err(|e| (0, format!("response parse failed: {e}")))?;
        if let Some(code) = json.get("error").and_then(|e| e.as_u64()) {
            let msg = json
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err((code as u32, msg));
        }
        Ok(json)
    }

    // Signed track submission: track.scrobble (with a timestamp) or
    // track.updateNowPlaying (without). Errors log; an invalid session
    // key triggers the background re-authorization.
    async fn submit(
        &self,
        method: &str,
        song: &ScrobbleSong,
        timestamp_ms: Option<i64>,
    ) -> Result<(), String> {
        // Read the session key fresh: a background re-authorization
        // stores a new key without rebuilding the reporters.
        let key = match (self.session_key)() {
            Ok(Some(k)) => k,
            Ok(None) => {
                tracing::warn!("lastfm: not authorized; {method} skipped");
                return Ok(());
            }
            Err(e) => {
                tracing::warn!("lastfm: session key unavailable: {e}");
                return Ok(());
            }
        };
        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("method", method.into());
        params.insert("api_key", self.api_key.clone());
        params.insert("sk", key);
        params.insert("artist", song.artist.clone());
        params.insert("track", song.title.clone());
        if let Some(ts) = timestamp_ms {
            params.insert("timestamp", (ts / 1000).to_string());
        }
        params.insert("duration", song.duration.to_string());
        if let Some(album) = &song.album {
            params.insert("album", album.clone());
        }
        let sig = self.sign(&params);
        params.insert("api_sig", sig);
        params.insert("format", "json".into());
        match self.post(params).await {
            Ok(_) => Ok(()),
            // 8: invalid session key; the user revoked access or
            // Last.fm invalidated the key. Re-authorize in the
            // background; the next submission uses the new key.
            Err((8, _msg)) => {
                tracing::warn!(
                    "lastfm: session key invalid; re-authorization started (check the terminal for the authorize URL)"
                );
                (self.reauth)(&self.api_key, &self.api_secret);
                Ok(())
            }
            Err((code, msg)) => {
                tracing::warn!("lastfm: API error {code}: {msg}");
                Ok(())
            }
        }
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
            self.submit("track.scrobble", song, Some(timestamp_ms))
                .await
        })
    }

    fn now_playing<'a>(
        &'a self,
        song: &'a ScrobbleSong,
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { self.submit("track.updateNowPlaying", song, None).await })
    }
}

// One re-authorization flow at a time. Spawns the interactive flow
// (prints the authorize URL, polls up to three minutes), which stores
// the new session key in the keychain on success.
fn trigger_reauthorization(api_key: &str, api_secret: &str) {
    static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return; // a flow is already running
    }
    let api_key = api_key.to_string();
    let api_secret = api_secret.to_string();
    tokio::spawn(async move {
        match lastfm_auth_flow(&api_key, &api_secret).await {
            Ok(()) => tracing::info!("lastfm: re-authorized; new session key stored"),
            Err(e) => tracing::warn!("lastfm: re-authorization failed: {e}"),
        }
        IN_FLIGHT.store(false, Ordering::SeqCst);
    });
}

// The Last.fm session key: OS keychain first, then the fallback file.
// A keyring that errors (headless Linux has no Secret Service) falls
// back to the file instead of failing.
pub fn lastfm_session_key() -> Result<Option<String>, String> {
    match keyring::Entry::new(KEYRING_SERVICE, LASTFM_KEYRING_USER).and_then(|e| e.get_password()) {
        Ok(sk) => Ok(Some(sk)),
        Err(keyring::Error::NoEntry) => read_key_file(),
        Err(e) => {
            tracing::debug!("keyring unavailable ({e}); using the session-key file");
            read_key_file()
        }
    }
}

// Store the Last.fm session key: OS keychain first, then the fallback
// file. A keyring set that fails (headless Linux without a Secret
// Service) writes the key file instead, so authorization still sticks.
fn store_lastfm_session_key(key: &str) -> Result<(), String> {
    let keyring =
        keyring::Entry::new(KEYRING_SERVICE, LASTFM_KEYRING_USER).and_then(|e| e.set_password(key));
    match keyring {
        Ok(()) => Ok(()),
        Err(e) => {
            let Some(path) = lastfm_key_file() else {
                return Err(format!("keyring set failed: {e}"));
            };
            tracing::warn!(
                "keyring unavailable ({e}); storing the Last.fm session key at {}",
                path.display()
            );
            write_key_file(&path, key).map_err(|e| format!("key file write failed: {e}"))
        }
    }
}

// Last.fm one-time auth, run automatically on startup when a [lastfm]
// block exists without a session key (and on demand via --lastfm-auth):
// getToken -> print the authorize URL -> poll getSession until the user
// authorizes (or the timeout passes) -> store the session key (keychain
// or fallback file). Never blocks forever: on timeout the caller
// continues without Last.fm scrobbling.
pub async fn lastfm_auth_flow(api_key: &str, api_secret: &str) -> Result<(), String> {
    const POLL_SECS: u64 = 3;
    const TIMEOUT_SECS: u64 = 180;
    const REMINDER_EVERY: u32 = 5; // polls

    let http = reqwest::Client::new();
    let token = lastfm_get_token(&http, api_key, api_secret).await?;
    println!(
        "Open {LASTFM_AUTH_URL}?api_key={api_key}&token={token} or scan the QR code below to authorize Subtidal with Last.fm."
    );

    let code = QrCode::new(format!("{LASTFM_AUTH_URL}?api_key={api_key}&token={token}"))
        .map_err(|e| format!("QR code generation failed: {e}"))?;
    let image = code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Dark)
        .light_color(unicode::Dense1x2::Light)
        .build();

    println!("{}", image);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(TIMEOUT_SECS);
    let mut polls: u32 = 0;
    loop {
        match lastfm_get_session(&http, api_key, api_secret, &token).await {
            Ok((key, name)) => {
                store_lastfm_session_key(&key)?;
                println!("Last.fm authenticated as {name}.");
                return Ok(());
            }
            Err((code, msg)) => {
                if classify_session_error(code) == SessionPoll::Fatal {
                    return Err(format!("error {code}: {msg}"));
                }
                if std::time::Instant::now() >= deadline {
                    return Err("timed out waiting for authorization".into());
                }
                polls += 1;
                if polls.is_multiple_of(REMINDER_EVERY) {
                    println!("Still waiting for Last.fm authorization (Ctrl-C to cancel)...");
                }
                tokio::time::sleep(std::time::Duration::from_secs(POLL_SECS)).await;
            }
        }
    }
}

// Signed form POST to the Last.fm API. The api_sig covers every param
// except format; format is appended after signing.
async fn lastfm_post(
    http: &reqwest::Client,
    params: &mut BTreeMap<&str, String>,
    api_secret: &str,
) -> Result<Value, String> {
    let mut sig_input = String::new();
    for (k, v) in params.iter() {
        if *k == "format" {
            continue;
        }
        sig_input.push_str(k);
        sig_input.push_str(v);
    }
    sig_input.push_str(api_secret);
    params.insert("api_sig", md5_hex(sig_input));
    params.insert("format", "json".into());
    http.post(LASTFM_API_URL)
        .form(params)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("response parse failed: {e}"))
}

async fn lastfm_get_token(
    http: &reqwest::Client,
    api_key: &str,
    api_secret: &str,
) -> Result<String, String> {
    let mut params: BTreeMap<&str, String> = BTreeMap::new();
    params.insert("method", "auth.getToken".into());
    params.insert("api_key", api_key.into());
    let body = lastfm_post(http, &mut params, api_secret).await?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(String::from)
        .ok_or_else(|| "auth.getToken: missing token".to_string())
}

// auth.getSession: Ok((session key, username)) once the user authorized
// the token; Err((code, message)) carries Last.fm's error code.
async fn lastfm_get_session(
    http: &reqwest::Client,
    api_key: &str,
    api_secret: &str,
    token: &str,
) -> Result<(String, String), (u32, String)> {
    let mut params: BTreeMap<&str, String> = BTreeMap::new();
    params.insert("method", "auth.getSession".into());
    params.insert("api_key", api_key.into());
    params.insert("token", token.into());
    let body = match lastfm_post(http, &mut params, api_secret).await {
        Ok(b) => b,
        Err(e) => return Err((0, e)),
    };
    if let Some(code) = body.get("error").and_then(|e| e.as_u64()) {
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error")
            .to_string();
        return Err((code as u32, msg));
    }
    let session = body
        .get("session")
        .ok_or_else(|| (0, "auth.getSession: missing session".to_string()))?;
    let key = session
        .get("key")
        .and_then(|k| k.as_str())
        .ok_or_else(|| (0, "auth.getSession: missing key".to_string()))?;
    let name = session
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| (0, "auth.getSession: missing name".to_string()))?;
    Ok((key.to_string(), name.to_string()))
}

// Poll outcome: config problems are fatal; anything else (the user has
// not authorized yet) keeps polling.
#[derive(Debug, PartialEq)]
enum SessionPoll {
    Pending,
    Fatal,
}

fn classify_session_error(code: u32) -> SessionPoll {
    match code {
        // 1: invalid service, 2: invalid method, 9: invalid API key,
        // 26: suspended API key. All are configuration errors.
        1 | 2 | 9 | 26 => SessionPoll::Fatal,
        _ => SessionPoll::Pending,
    }
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

    // Listen metadata shared by completed and now-playing listens.
    fn metadata(&self, song: &ScrobbleSong) -> Value {
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
        metadata
    }

    // POST one listen; 2xx is success, the rest map to a message.
    async fn submit_listen(&self, body: Value) -> Result<(), String> {
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
            let body = serde_json::json!({
                "listen_type": "single",
                "payload": [{
                    "listened_at": timestamp_ms / 1000,
                    "track_metadata": self.metadata(song),
                }],
            });
            self.submit_listen(body).await
        })
    }

    fn now_playing<'a>(
        &'a self,
        song: &'a ScrobbleSong,
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let body = serde_json::json!({
                "listen_type": "playing_now",
                "payload": [{
                    "track_metadata": self.metadata(song),
                }],
            });
            self.submit_listen(body).await
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
        let reporter = LastFmReporter::new_with_fakes(
            "key123".into(),
            "secret".into(),
            LASTFM_API_URL,
            Box::new(|| Ok(None)),
            Box::new(|_, _| ()),
        );
        let input = "api_keykey123artistTaylor Swiftmethodtrack.scrobble".to_string() + "secret";
        assert_eq!(reporter.sign(&params), md5_hex(&input));
    }

    #[test]
    fn session_errors_fatal_only_for_config_problems() {
        assert_eq!(classify_session_error(1), SessionPoll::Fatal);
        assert_eq!(classify_session_error(2), SessionPoll::Fatal);
        assert_eq!(classify_session_error(9), SessionPoll::Fatal);
        assert_eq!(classify_session_error(26), SessionPoll::Fatal);
        // Not-yet-authorized, temporary, and service errors keep polling.
        assert_eq!(classify_session_error(4), SessionPoll::Pending);
        assert_eq!(classify_session_error(10), SessionPoll::Pending);
        assert_eq!(classify_session_error(13), SessionPoll::Pending);
        assert_eq!(classify_session_error(14), SessionPoll::Pending);
        assert_eq!(classify_session_error(16), SessionPoll::Pending);
    }

    #[test]
    fn session_key_file_roundtrips() {
        let dir = std::env::temp_dir().join(format!("subtidal-key-test-{}", std::process::id()));
        let path = dir.join("lastfm-session-key");
        // A missing file reads as None.
        assert_eq!(read_key_file_at(&path).unwrap(), None);
        write_key_file(&path, "sk123").unwrap();
        assert_eq!(read_key_file_at(&path).unwrap(), Some("sk123".to_string()));
        // The file must be private to the user.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(&dir).ok();
    }

    // A recording reporter for fan-out tests.
    struct RecordingReporter {
        calls: Arc<Mutex<Vec<(u64, i64)>>>,
        now_playing_calls: Arc<Mutex<Vec<u64>>>,
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

        fn now_playing<'a>(
            &'a self,
            song: &'a ScrobbleSong,
        ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                self.now_playing_calls.lock().unwrap().push(song.track_id);
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
                now_playing_calls: Arc::new(Mutex::new(Vec::new())),
                fail: false,
            }),
            Box::new(RecordingReporter {
                calls: calls.clone(),
                now_playing_calls: Arc::new(Mutex::new(Vec::new())),
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

    #[test]
    fn now_playing_dedup_depends_on_track_and_cooldown() {
        assert!(np_due(1, 1000, &None));
        let last = Some(LastNowPlaying {
            track_id: 1,
            at_ms: 1000,
        });
        // Same track inside the cooldown: suppressed.
        assert!(!np_due(1, 1000 + NP_COOLDOWN_MS - 1, &last));
        // Same track after the cooldown: due again (repeat play).
        assert!(np_due(1, 1000 + NP_COOLDOWN_MS + 1, &last));
        // A different track is always due.
        assert!(np_due(2, 1001, &last));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn report_now_playing_song_fans_out_to_all_reporters() {
        let _g = registry_lock();
        let nps = Arc::new(Mutex::new(Vec::new()));
        *registry().lock().unwrap() = Arc::new(vec![
            Box::new(RecordingReporter {
                calls: Arc::new(Mutex::new(Vec::new())),
                now_playing_calls: nps.clone(),
                fail: false,
            }),
            Box::new(RecordingReporter {
                calls: Arc::new(Mutex::new(Vec::new())),
                now_playing_calls: nps.clone(),
                fail: true,
            }),
        ]);
        report_now_playing_song(&scrobble_song()).await;
        let nps = nps.lock().unwrap();
        assert_eq!(
            nps.len(),
            2,
            "a failing reporter must not stop the now-playing fan-out"
        );
        assert!(nps.iter().all(|id| *id == 463900374));
    }

    // Spin up a local warp server on an ephemeral port. Every request is
    // captured (raw body) and answered 200 with "ok".
    async fn mock_server() -> (String, Arc<Mutex<Vec<String>>>) {
        mock_server_json(serde_json::json!("ok")).await
    }

    // Like mock_server, but answers every request with the given JSON.
    async fn mock_server_json(body: Value) -> (String, Arc<Mutex<Vec<String>>>) {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        let route = warp::body::bytes().map(move |bytes: bytes::Bytes| {
            cap.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&bytes).to_string());
            let reply = body.clone();
            warp::reply::with_status(warp::reply::json(&reply), warp::http::StatusCode::OK)
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
        let reporter = LastFmReporter::new_with_fakes(
            "key".into(),
            "secret".into(),
            &format!("{base}/2.0/"),
            Box::new(|| Ok(Some("sk123".into()))),
            Box::new(|_, _| ()),
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
        let reporter = LastFmReporter::new_with_fakes(
            "key".into(),
            "secret".into(),
            &format!("{base}/nope"),
            Box::new(|| Ok(Some("sk".into()))),
            Box::new(|_, _| ()),
        );
        assert!(reporter.report(&scrobble_song(), 0).await.is_ok());
    }

    #[tokio::test]
    async fn lastfm_report_reauths_on_invalid_session_key() {
        let (base, _captured) = mock_server_json(serde_json::json!({
            "error": 8,
            "message": "Invalid session key - Please re-authenticate"
        }))
        .await;
        let reauths: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let reauths2 = reauths.clone();
        let reporter = LastFmReporter::new_with_fakes(
            "key".into(),
            "secret".into(),
            &format!("{base}/2.0/"),
            Box::new(|| Ok(Some("sk".into()))),
            Box::new(move |k, s| {
                reauths2
                    .lock()
                    .unwrap()
                    .push((k.to_string(), s.to_string()))
            }),
        );
        assert!(reporter.report(&scrobble_song(), 0).await.is_ok());
        let reauths = reauths.lock().unwrap();
        assert_eq!(reauths.len(), 1);
        assert_eq!(reauths[0], ("key".to_string(), "secret".to_string()));
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
