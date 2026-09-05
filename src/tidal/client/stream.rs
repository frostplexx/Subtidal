// Stream metadata: trackManifests decoding. Backs the stream endpoint,
// which proxies Tidal's own HLS manifest: v2 answers manifestType=HLS
// with the exact playlists the official app downloads from, so no DASH
// translation happens here.
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use regex::Regex;
use serde_json::Value;
use tokio::sync::{Semaphore, SemaphorePermit};

use super::{Error, TidalClient, OPENAPI_URL};

// Tidal's native HLS manifest for one track: the media playlist text as
// the API serves it, plus the format metadata this server logs. The CDN
// URLs inside carry short-lived signed tokens, so nothing here is
// cached.
#[derive(Clone)]
pub struct HlsInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: u8,
    pub media_playlist: String,
}

// Stream metadata for one track.
pub struct StreamInfo {
    // Kept for shape parity and tests; the handlers no longer branch on
    // mime type (v2 always answers HLS).
    #[allow(dead_code)]
    pub mime_type: String,
    // Direct single-file URL when the manifest is plain https
    // (uriScheme=DATA should prevent it; defended anyway).
    pub direct_url: Option<String>,
    pub hls: Option<HlsInfo>,
}

// Caps on playbackinfo fetches. A client bursting stream URLs (a
// downloader fetches the whole queue at once) is throttled here
// instead of spamming the Tidal API, which fails with decode errors
// under parallel load. At most STREAM_LIMIT fetches run at once, and
// at most STREAM_WINDOW_MAX start within STREAM_WINDOW; a start that
// would exceed either waits up to STREAM_WAIT for a slot, then is
// rejected with RateLimited.
//
// Tidal can also throttle the account for a while (non-JSON bodies,
// 429, 5xx). The circuit breaker pauses all starts for
// THROTTLE_COOLDOWN after THROTTLE_TRIGGER consecutive such failures,
// so the account throttle clears instead of being re-armed by the
// steady drain.
const STREAM_LIMIT: usize = 3;
const STREAM_WINDOW: Duration = Duration::from_secs(10);
const STREAM_WINDOW_MAX: usize = 5;
// Bounded wait for a slot. At the window pace a whole download queue
// passes (5 starts per 10 s drain 30 tracks a minute); the bound trips
// only on absurd bursts. It also guards against a hung fetch holding a
// permit forever.
const STREAM_WAIT: Duration = Duration::from_secs(600);
// Circuit breaker: trigger and pause lengths.
const THROTTLE_TRIGGER: u32 = 3;
const THROTTLE_COOLDOWN: Duration = Duration::from_secs(60);

// Outcome of one playbackinfo fetch, fed back to the limiter.
#[derive(Clone, Copy, PartialEq)]
enum FetchOutcome {
    Success,
    // Non-JSON body, 429, or 5xx: the account-level throttle signature.
    Throttled,
    // Any other error (auth, 403, 404, parse): not throttle evidence.
    Other,
}

// True for the account-throttle signature: a non-JSON body (decode
// error), 429, or 5xx. The download-mode fallback must not retry
// during a throttle.
fn throttle_signature(e: &Error) -> bool {
    match e {
        Error::Http(e) => e.is_decode(),
        Error::HttpDecode(_, _) => true,
        Error::Tidal(status, _) => *status == 429 || (500..600).contains(status),
        _ => false,
    }
}

struct LimiterState {
    consecutive_failures: u32,
    cooldown_until: Option<Instant>,
}

pub(crate) struct StreamLimiter {
    semaphore: Semaphore,
    recent: Mutex<VecDeque<Instant>>,
    state: Mutex<LimiterState>,
}

impl StreamLimiter {
    pub(crate) fn new() -> Self {
        Self {
            semaphore: Semaphore::new(STREAM_LIMIT),
            recent: Mutex::new(VecDeque::new()),
            state: Mutex::new(LimiterState {
                consecutive_failures: 0,
                cooldown_until: None,
            }),
        }
    }

    // Wait for the concurrency permit and a window slot, bounded by
    // STREAM_WAIT. The caller holds the permit across the fetch. A
    // start that waited too long is rejected with RateLimited; only a
    // hung fetch or an absurd burst makes the wait expire.
    pub(crate) async fn acquire(&self) -> Result<SemaphorePermit<'_>, Error> {
        let deadline = Instant::now() + STREAM_WAIT;
        let permit = match tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.semaphore.acquire(),
        )
        .await
        {
            Ok(Ok(p)) => p,
            _ => return Err(Error::RateLimited),
        };
        loop {
            // An active throttle pause holds every start; the guard
            // must end before the sleep below, so scope it.
            let wait = {
                let state = self.state.lock().unwrap();
                match state.cooldown_until {
                    Some(until) => until.saturating_duration_since(Instant::now()),
                    None => Duration::ZERO,
                }
            };
            if wait > Duration::ZERO {
                return Err(Error::RateLimited);
            }
            // The window is full; wait until the oldest start ages out.
            let wait = {
                let mut recent = self.recent.lock().unwrap();
                let now = Instant::now();
                if window_allows(&mut recent, now) {
                    return Ok(permit);
                }
                recent
                    .front()
                    .map(|t| (*t + STREAM_WINDOW).saturating_duration_since(now))
                    .unwrap_or_default()
            };
            if Instant::now() + wait >= deadline {
                return Err(Error::RateLimited);
            }
            tokio::time::sleep(wait.max(Duration::from_millis(50))).await;
        }
    }

    // Record one fetch outcome. Throttle-signature failures count
    // toward the trigger; at THROTTLE_TRIGGER the pause starts. An
    // active pause swallows every result: nothing new goes out, so an
    // in-flight leftover must neither extend nor clear the pause. An
    // expired pause is a clean slate for the next cycle.
    fn note(&self, outcome: FetchOutcome) {
        let mut state = self.state.lock().unwrap();
        if state.cooldown_until.is_some_and(|until| Instant::now() < until) {
            return;
        }
        state.cooldown_until = None;
        match outcome {
            FetchOutcome::Success => state.consecutive_failures = 0,
            FetchOutcome::Throttled => {
                state.consecutive_failures += 1;
                if state.consecutive_failures >= THROTTLE_TRIGGER {
                    tracing::warn!(
                        "tidal is throttling stream requests; pausing for {}s",
                        THROTTLE_COOLDOWN.as_secs()
                    );
                    state.cooldown_until = Some(Instant::now() + THROTTLE_COOLDOWN);
                    state.consecutive_failures = 0;
                }
            }
            FetchOutcome::Other => {}
        }
    }
}

// True when a new stream start fits the sliding window: fewer than
// STREAM_WINDOW_MAX starts within the last STREAM_WINDOW. Expired
// starts are pruned first; a passed start is recorded.
fn window_allows(recent: &mut VecDeque<Instant>, now: Instant) -> bool {
    let cutoff = now - STREAM_WINDOW;
    while recent.front().is_some_and(|t| *t < cutoff) {
        recent.pop_front();
    }
    if recent.len() >= STREAM_WINDOW_MAX {
        false
    } else {
        recent.push_back(now);
        true
    }
}

// One UUID v4 per stream fetch. Tidal's edge expects a playback-session
// header on stream requests; the official app sends one per download.
fn new_session_id() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let mut out = String::with_capacity(36);
    for (i, byte) in b.iter().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            out.push('-');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

impl TidalClient {
    // Fetch a track manifest at the given quality tier. Never cached:
    // the CDN URLs carry short-lived signed tokens. v2 has no BTS and no
    // quality tiers: the manifest always answers HLS, so the tiers only
    // widen the format list.
    pub async fn stream_info(&self, track_id: u64, quality: &str, mode: &str) -> Result<StreamInfo, Error> {
        // Throttle: wait (bounded) for a concurrency and window slot.
        // The permit stays held across the HTTP call.
        let _permit = self.stream_limiter.acquire().await?;
        let token = self.access_token().await?;
        // One playback session per fetch, like the official app, which
        // sends X-Playback-Session-Id on every request of a download.
        let session_id = new_session_id();
        let formats = audio_quality_to_formats(quality);
        let result = async {
            let resp = self
                .http
                .get(format!("{}/trackManifests/{track_id}", OPENAPI_URL))
                .bearer_auth(token)
                .header("x-tidal-client-version", super::CLIENT_VERSION)
                .header("X-Playback-Session-Id", session_id)
                .query(&[
                    ("manifestType", "HLS"),
                    ("formats", formats),
                    ("uriScheme", "DATA"),
                    ("usage", if mode == "OFFLINE" { "DOWNLOAD" } else { "PLAYBACK" }),
                    ("adaptive", "false"),
                ])
                .send()
                .await?;
            let status = resp.status();
            // Read the raw body first. resp.json() would discard the
            // text on a decode failure, but a throttled response is
            // HTML or empty, and that text is the diagnostic.
            let text = resp.text().await?;
            let body: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => return Err(Error::HttpDecode(status.as_u16(), text)),
            };
            if !status.is_success() {
                return Err(Error::Tidal(status.as_u16(), body.to_string()));
            }
            parse_manifest(body)
        }
        .await;
        // Feed the circuit breaker: non-JSON bodies, 429, and 5xx are
        // the account-throttle signature. Everything else is neutral.
        let outcome = match &result {
            Ok(_) => FetchOutcome::Success,
            Err(e) if throttle_signature(e) => FetchOutcome::Throttled,
            _ => FetchOutcome::Other,
        };
        self.stream_limiter.note(outcome);
        result
    }

    // A download asks for the offline manifest first, like the official
    // app (its CDN URLs carry an info=DOWNLOAD tag). A mode rejection,
    // for example no offline entitlement, falls back to the streaming
    // mode; a throttle-signature or queue-full failure does not,
    // because a retry under the same conditions fails the same way.
    pub(crate) async fn download_info(
        &self,
        track_id: u64,
        quality: &str,
    ) -> Result<StreamInfo, Error> {
        match self.stream_info(track_id, quality, "OFFLINE").await {
            Ok(info) => Ok(info),
            Err(e) if throttle_signature(&e) || matches!(e, Error::RateLimited) => Err(e),
            Err(e) => {
                tracing::debug!(
                    "offline mode unavailable for track {track_id} ({e}); retrying STREAM"
                );
                self.stream_info(track_id, quality, "STREAM").await
            }
        }
    }

}

// The format set per quality tier, mirroring the SDK's audioQualityToFormats.
fn audio_quality_to_formats(quality: &str) -> &'static str {
    match quality {
        "ATMOS" => "EAC3_JOC,FLAC_HIRES,FLAC",
        "HI_RES" => "HEAACV1,AACLC,FLAC,FLAC_HIRES",
        "LOSSLESS" => "HEAACV1,AACLC,FLAC",
        "HIGH" => "HEAACV1,AACLC",
        _ => "HEAACV1",
    }
}

// Decode a v2 trackManifests document: attributes.uri is a base64 data
// URI carrying Tidal's HLS master playlist, whose single variant is an
// inline base64 media playlist (uriScheme=DATA). A plain https uri
// (should not happen, but defended) becomes a direct stream URL.
fn parse_manifest(body: Value) -> Result<StreamInfo, Error> {
    let attrs = &body["data"]["attributes"];
    let uri = attrs["uri"]
        .as_str()
        .ok_or_else(|| Error::Auth("response missing manifest".into()))?;
    if let Some(url) = uri.strip_prefix("https://") {
        return Ok(StreamInfo {
            mime_type: "application/vnd.tidal.bts".into(),
            direct_url: Some(format!("https://{url}")),
            hls: None,
        });
    }
    let b64 = uri
        .strip_prefix("data:application/vnd.apple.mpegurl;base64,")
        .ok_or_else(|| Error::Auth("unexpected manifest uri".into()))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64))
        .map_err(|e| Error::Auth(format!("manifest decode failed: {e}")))?;
    let master = String::from_utf8_lossy(&decoded).into_owned();
    let hls = parse_hls_master(&master)
        .ok_or_else(|| Error::Auth("manifest carries no playable variant".into()))?;
    Ok(StreamInfo {
        mime_type: "application/vnd.apple.mpegurl".into(),
        direct_url: None,
        hls: Some(hls),
    })
}

static RE_CODECS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"CODECS="([^"]*)""#).unwrap());

// The v2 HLS master is a single-variant playlist: each STREAM-INF line
// is followed directly by its playlist URI, a base64 data URI
// (uriScheme=DATA). Tidal serves one variant, the first.
fn parse_hls_master(master: &str) -> Option<HlsInfo> {
    let mut lines = master.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with("#EXT-X-STREAM-INF:") {
            continue;
        }
        let codec = RE_CODECS
            .captures(line)
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        let media = decode_playlist_uri(lines.next()?.trim())?;
        let (sample_rate, bit_depth) = daterange_metadata(&media);
        return Some(HlsInfo {
            codec,
            sample_rate,
            bit_depth,
            media_playlist: media,
        });
    }
    None
}

// Decode one inline playlist reference (a data URI). Any other scheme
// means the manifest shape changed; treat it as absent.
fn decode_playlist_uri(uri: &str) -> Option<String> {
    let b64 = uri.strip_prefix("data:application/vnd.apple.mpegurl;base64,")?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64))
        .ok()?;
    Some(String::from_utf8_lossy(&raw).into_owned())
}

// The media playlist's DATERANGE carries the format metadata
// (X-COM-TIDAL-SAMPLE-RATE/DEPTH), which feeds the stream log line
// only; a missing range yields zeros.
fn daterange_metadata(media: &str) -> (u32, u8) {
    let Some(line) = media.lines().find(|l| l.starts_with("#EXT-X-DATERANGE:")) else {
        return (0, 0);
    };
    let (mut rate, mut depth) = (0, 0);
    for attr in line.split(',') {
        if let Some(v) = attr.strip_prefix("X-COM-TIDAL-SAMPLE-RATE=") {
            rate = v.trim_matches('"').parse().unwrap_or(0);
        } else if let Some(v) = attr.strip_prefix("X-COM-TIDAL-SAMPLE-DEPTH=") {
            depth = v.trim_matches('"').parse().unwrap_or(0);
        }
    }
    (rate, depth)
}



#[cfg(test)]
mod tests {
    use super::*;

    // Real shape of the v2 HLS manifest (captured live), URLs shortened:
    // a one-variant master whose media playlist is an inline data URI.
    const MEDIA: &str = "#EXTM3U\n\
#EXT-X-VERSION:7\n\
#EXT-X-PLAYLIST-TYPE:VOD\n\
#EXT-X-DATERANGE:ID=\"d\",START-DATE=\"2024-01-01T00:00:00.000Z\",X-COM-TIDAL-FORMAT=\"FLAC\",X-COM-TIDAL-SAMPLE-RATE=44100,X-COM-TIDAL-SAMPLE-DEPTH=16\n\
#EXT-X-MAP:URI=\"https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/0.mp4?token=T\"\n\
#EXTINF:3.994,\n\
https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/1.mp4?token=T\n\
#EXT-X-ENDLIST\n";

    // Same media playlist without the DATERANGE metadata block.
    const MEDIA_NOMETA: &str = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-MAP:URI=\"u\"\n#EXTINF:4.0,\nv\n#EXT-X-ENDLIST\n";

    fn hls_master_bytes() -> String {
        let variant = base64::engine::general_purpose::STANDARD.encode(MEDIA);
        format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:BANDWIDTH=890170,AVERAGE-BANDWIDTH=711268,CODECS=\"fLaC\"\ndata:application/vnd.apple.mpegurl;base64,{variant}\n"
        )
    }

    #[test]
    fn unavailable_asset_is_definitive_and_not_throttle() {
        let err = Error::Tidal(
            401,
            r#"{"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#
                .into(),
        );
        assert!(err.is_unavailable_asset());
        assert!(!throttle_signature(&err));
        assert!(!Error::Tidal(401, r#"{"status":401,"subStatus":1002}"#.into()).is_unavailable_asset());
        assert!(!Error::Tidal(404, "not found".into()).is_unavailable_asset());
    }

    #[test]
    fn decode_error_display_keeps_status_and_body() {
        let empty = Error::HttpDecode(200, String::new());
        assert_eq!(empty.to_string(), "tidal answered 200 with an empty body");
        let html = Error::HttpDecode(403, "<html>blocked</html>".into());
        assert!(html.to_string().contains("403"));
        assert!(html.to_string().contains("<html>blocked</html>"));
    }

    #[test]
    fn throttle_signature_matches_only_throttle_errors() {
        assert!(throttle_signature(&Error::HttpDecode(200, "<html>".into())));
        assert!(throttle_signature(&Error::Tidal(429, String::new())));
        assert!(throttle_signature(&Error::Tidal(503, String::new())));
        assert!(!throttle_signature(&Error::Tidal(400, String::new())));
        assert!(!throttle_signature(&Error::Tidal(404, String::new())));
        assert!(!throttle_signature(&Error::Auth("x".into())));
        assert!(!throttle_signature(&Error::RateLimited));
        assert!(!throttle_signature(&Error::NotLoggedIn));
    }

    #[test]
    fn window_allows_five_per_ten_seconds() {
        let mut recent = VecDeque::new();
        let t0 = Instant::now();
        for i in 0..STREAM_WINDOW_MAX as u64 {
            assert!(
                window_allows(&mut recent, t0 + Duration::from_millis(i)),
                "start {i} must pass"
            );
        }
        // One more start inside the window is rejected.
        assert!(!window_allows(&mut recent, t0 + Duration::from_secs(9)));
        // Once the first start is older than the window, a new one passes.
        assert!(window_allows(
            &mut recent,
            t0 + STREAM_WINDOW + Duration::from_millis(1)
        ));
    }

    #[test]
    fn cooldown_triggers_after_consecutive_throttle_failures() {
        let limiter = StreamLimiter::new();
        for _ in 0..THROTTLE_TRIGGER - 1 {
            limiter.note(FetchOutcome::Throttled);
            assert!(
                limiter.state.lock().unwrap().cooldown_until.is_none(),
                "below the trigger the pause must not start"
            );
        }
        limiter.note(FetchOutcome::Throttled);
        assert!(limiter.state.lock().unwrap().cooldown_until.is_some());
        // A success during an active pause cannot clear it.
        limiter.note(FetchOutcome::Success);
        assert!(limiter.state.lock().unwrap().cooldown_until.is_some());
        // An expired pause is a clean slate for the next cycle.
        {
            let mut state = limiter.state.lock().unwrap();
            state.cooldown_until = Some(Instant::now() - Duration::from_millis(1));
        }
        limiter.note(FetchOutcome::Success);
        let state = limiter.state.lock().unwrap();
        assert!(state.cooldown_until.is_none());
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn session_id_is_a_uuid_v4() {
        let id = new_session_id();
        assert_eq!(id.len(), 36);
        let bytes = id.as_bytes();
        for (i, c) in bytes.iter().enumerate() {
            if i == 8 || i == 13 || i == 18 || i == 23 {
                assert_eq!(*c, b'-');
            } else {
                assert!(c.is_ascii_hexdigit());
            }
        }
        assert_eq!(&id[14..15], "4");
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }

    #[test]
    fn parses_hires_hls() {
        let d = parse_hls_master(&hls_master_bytes()).expect("parses");
        assert_eq!(d.codec, "fLaC");
        assert_eq!(d.sample_rate, 44100);
        assert_eq!(d.bit_depth, 16);
        assert_eq!(d.media_playlist, MEDIA);
    }

    #[test]
    fn picks_the_first_stream_inf_line() {
        // A second variant line follows the first's data URI; the first
        // wins and its https variant ref is never followed.
        let m = hls_master_bytes().replace(
            "#EXT-X-STREAM-INF:BANDWIDTH=890170,AVERAGE-BANDWIDTH=711268,CODECS=\"fLaC\"\ndata:",
            "#EXT-X-STREAM-INF:BANDWIDTH=111,CODECS=\"ec-3\"\ndata:",
        );
        let m = format!(
            "{m}#EXT-X-STREAM-INF:BANDWIDTH=890170,CODECS=\"fLaC\"\nhttps://cdn/second.m3u8\n"
        );
        let d = parse_hls_master(&m).expect("parses");
        assert_eq!(d.codec, "ec-3");
        assert_eq!(d.media_playlist, MEDIA);
    }

    #[test]
    fn parses_v2_hls_manifest_document() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(hls_master_bytes());
        let body: Value = serde_json::json!({
            "data": {
                "type": "trackManifests",
                "id": "7",
                "attributes": {
                    "uri": format!("data:application/vnd.apple.mpegurl;base64,{b64}"),
                    "formats": ["HEAACV1", "AACLC", "FLAC", "FLAC_HIRES"],
                },
            },
        });
        let info = parse_manifest(body).unwrap();
        assert_eq!(info.mime_type, "application/vnd.apple.mpegurl");
        assert!(info.direct_url.is_none());
        let hls = info.hls.expect("hls parsed");
        assert_eq!(hls.codec, "fLaC");
        assert_eq!(hls.sample_rate, 44100);
        assert_eq!(hls.bit_depth, 16);
        // The media playlist passes through byte-for-byte.
        assert_eq!(hls.media_playlist, MEDIA);
        assert!(hls.media_playlist.contains("X-COM-TIDAL-FORMAT=\"FLAC\""));
        assert!(hls.media_playlist.contains(
            "#EXT-X-MAP:URI=\"https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/0.mp4?token=T\""
        ));
    }

    #[test]
    fn parses_v2_https_uri_as_direct_url() {
        let body: Value = serde_json::json!({
            "data": { "attributes": { "uri": "https://cdn/1.mp4?token=x" } }
        });
        let info = parse_manifest(body).unwrap();
        assert_eq!(info.direct_url.as_deref(), Some("https://cdn/1.mp4?token=x"));
        assert!(info.hls.is_none());
    }

    #[test]
    fn master_missing_codecs_or_daterange_still_parses() {
        let variant = base64::engine::general_purpose::STANDARD.encode(MEDIA_NOMETA);
        let master = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:BANDWIDTH=890170\ndata:application/vnd.apple.mpegurl;base64,{variant}\n"
        );
        let hls = parse_hls_master(&master).expect("parses");
        assert_eq!(hls.codec, "");
        assert_eq!(hls.sample_rate, 0);
        assert_eq!(hls.bit_depth, 0);
        assert_eq!(hls.media_playlist, MEDIA_NOMETA);
    }

    #[test]
    fn non_data_playlist_refs_are_rejected() {
        assert!(decode_playlist_uri("https://cdn/pl.m3u8?token=x").is_none());
        assert!(decode_playlist_uri("data:application/dash+xml;base64,AAAA").is_none());
    }

    #[test]
    fn formats_follow_tier() {
        assert_eq!(audio_quality_to_formats("HI_RES"), "HEAACV1,AACLC,FLAC,FLAC_HIRES");
        assert_eq!(audio_quality_to_formats("LOSSLESS"), "HEAACV1,AACLC,FLAC");
        assert_eq!(audio_quality_to_formats("HIGH"), "HEAACV1,AACLC");
        assert_eq!(audio_quality_to_formats("LOW"), "HEAACV1");
    }

    #[test]
    fn limiter_caps_concurrency_at_limit() {
        let limiter = StreamLimiter::new();
        let permits: Vec<_> = (0..STREAM_LIMIT)
            .map(|_| limiter.semaphore.try_acquire().expect("slot free"))
            .collect();
        assert!(
            limiter.semaphore.try_acquire().is_err(),
            "a {}th concurrent fetch must be rejected",
            STREAM_LIMIT + 1
        );
        drop(permits);
        assert!(limiter.semaphore.try_acquire().is_ok());
    }
}
