// Stream metadata: trackManifests decoding. Backs the stream endpoint,
// which rewrites the DASH manifest into an HLS playlist of Tidal's own
// CDN segment URLs. v2 always answers MPEG_DASH, so every stream is
// served as HLS.
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use regex::Regex;
use serde_json::Value;
use tokio::sync::{Semaphore, SemaphorePermit};

use super::{Error, TidalClient, API_URL, OPENAPI_URL};

// One run of equal-length segments from the DASH SegmentTimeline.
// count is r+1: DASH repeats a duration r extra times.
pub struct Segment {
    pub samples: u64,
    pub count: u32,
}

// Parsed hi-res DASH manifest. The CDN URLs carry short-lived tokens, so
// nothing here is cached.
pub struct DashInfo {
    pub bandwidth: u64,
    pub init_url: String,
    // Segment URL with a $Number$ placeholder to substitute.
    pub media_url: String,
    pub timescale: u64,
    pub start_number: u64,
    pub segments: Vec<Segment>,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: u8,
}

// Stream metadata for one track.
pub struct StreamInfo {
    // Kept for shape parity and tests; the handlers no longer branch on
    // mime type (v2 always answers DASH).
    #[allow(dead_code)]
    pub mime_type: String,
    // Direct single-file URL when the manifest is playable as one file
    // (BTS). Segmented DASH (hi-res FLAC) has no such URL; the stream
    // handler serves an HLS playlist instead, or falls back a tier.
    pub direct_url: Option<String>,
    pub dash: Option<DashInfo>,
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
const STREAM_LIMIT: usize = 5;
const STREAM_WINDOW: Duration = Duration::from_secs(5);
const STREAM_WINDOW_MAX: usize = 5;
// Bounded wait for a slot. Large enough that a downloader's whole
// queue passes (12 starts per 5 s drains ~140 tracks in a minute);
// the bound trips only on absurd bursts.
const STREAM_WAIT: Duration = Duration::from_secs(60);
// Circuit breaker: trigger and pause lengths.
const THROTTLE_TRIGGER: u32 = 5;
const THROTTLE_COOLDOWN: Duration = Duration::from_secs(30);

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
// error), 429, or 5xx. Shared by the outcome classifier and the
// download-mode fallback, which must not retry during a throttle.
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
    // start that waited too long is rejected with RateLimited.
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
                if Instant::now() + wait >= deadline {
                    return Err(Error::RateLimited);
                }
                tokio::time::sleep(wait).await;
                continue;
            }
            // The window is full; wait until the oldest start ages out.
            let wait = {
                let mut recent = self.recent.lock().unwrap();
                let now = Instant::now();
                if window_allows(&mut recent, now) {
                    return Ok(permit);
                }
                // The window is full; wait until the oldest start ages out.
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
    // quality tiers: the manifest always answers MPEG_DASH, so the tiers
    // only widen the format list. The tier-retry loop lives on in
    // stream_info_v1 and is not needed here.
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
                    ("manifestType", "MPEG_DASH"),
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
    // mode; a throttle-signature failure does not, because retrying the
    // same token under an active throttle only re-arms it.
    pub(crate) async fn download_info(
        &self,
        track_id: u64,
        quality: &str,
    ) -> Result<StreamInfo, Error> {
        match self.stream_info(track_id, quality, "OFFLINE").await {
            Ok(info) => Ok(info),
            Err(e) if throttle_signature(&e) => Err(e),
            Err(e) => {
                tracing::debug!(
                    "offline mode unavailable for track {track_id} ({e}); retrying STREAM"
                );
                self.stream_info(track_id, quality, "STREAM").await
            }
        }
    }

    // --- v1 backup (dead code) -------------------------------------
    #[allow(dead_code)]
    pub async fn stream_info_v1(&self, track_id: u64, quality: &str, mode: &str) -> Result<StreamInfo, Error> {
        let _permit = self.stream_limiter.acquire().await?;
        let token = self.access_token().await?;
        let cc = self.country_code().await?;
        let session_id = new_session_id();
        let mut query = vec![
            ("audioquality", quality),
            ("playbackmode", mode),
            ("assetpresentation", "FULL"),
        ];
        if let Some(cc) = &cc {
            query.push(("countryCode", cc.as_str()));
        }
        let result = async {
            let resp = self
                .http
                .get(format!("{API_URL}/tracks/{track_id}/playbackinfopostpaywall"))
                .bearer_auth(token)
                .header("x-tidal-client-version", super::CLIENT_VERSION)
                .header("X-Playback-Session-Id", session_id)
                .query(&query)
                .send()
                .await?;
            let status = resp.status();
            let text = resp.text().await?;
            let body: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => return Err(Error::HttpDecode(status.as_u16(), text)),
            };
            if !status.is_success() {
                return Err(Error::Tidal(status.as_u16(), body.to_string()));
            }
            parse_stream_v1(body)
        }
        .await;
        let outcome = match &result {
            Ok(_) => FetchOutcome::Success,
            Err(e) if throttle_signature(e) => FetchOutcome::Throttled,
            _ => FetchOutcome::Other,
        };
        self.stream_limiter.note(outcome);
        result
    }
}

// The format set per quality tier, mirroring the SDK's audioQualityToFormats.
fn audio_quality_to_formats(quality: &str) -> &'static str {
    match quality {
        "HI_RES" => "HEAACV1,AACLC,FLAC,FLAC_HIRES",
        "LOSSLESS" => "HEAACV1,AACLC,FLAC",
        "HIGH" => "HEAACV1,AACLC",
        _ => "HEAACV1",
    }
}

// Decode a v2 trackManifests document: attributes.uri is a base64 data
// URI carrying the MPD. A plain https uri (uriScheme=DATA should prevent
// it, but defensively) becomes a direct stream URL.
fn parse_manifest(body: Value) -> Result<StreamInfo, Error> {
    let attrs = &body["data"]["attributes"];
    let uri = attrs["uri"]
        .as_str()
        .ok_or_else(|| Error::Auth("response missing manifest".into()))?;
    if let Some(url) = uri.strip_prefix("https://") {
        return Ok(StreamInfo {
            mime_type: "application/vnd.tidal.bts".into(),
            direct_url: Some(format!("https://{url}")),
            dash: None,
        });
    }
    let b64 = uri
        .strip_prefix("data:application/dash+xml;base64,")
        .ok_or_else(|| Error::Auth("unexpected manifest uri".into()))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64))
        .map_err(|e| Error::Auth(format!("manifest decode failed: {e}")))?;
    let manifest = String::from_utf8_lossy(&decoded).into_owned();
    let dash = parse_dash(&manifest);
    Ok(StreamInfo {
        mime_type: "application/dash+xml".into(),
        direct_url: None,
        dash,
    })
}

#[allow(dead_code)]
fn parse_stream_v1(body: Value) -> Result<StreamInfo, Error> {
    let mime = body["manifestMimeType"].as_str().unwrap_or("").to_string();
    let manifest_b64 = body["manifest"]
        .as_str()
        .ok_or_else(|| Error::Auth("response missing manifest".into()))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(manifest_b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(manifest_b64))
        .map_err(|e| Error::Auth(format!("manifest decode failed: {e}")))?;
    let manifest = String::from_utf8_lossy(&decoded).into_owned();
    let direct_url = if mime.contains("bts") {
        serde_json::from_str::<Value>(&manifest)
            .ok()
            .and_then(|v| v["urls"].get(0).and_then(|u| u.as_str()).map(String::from))
    } else {
        None
    };
    let dash = if mime.contains("dash") {
        parse_dash(&manifest)
    } else {
        None
    };
    Ok(StreamInfo {
        mime_type: mime,
        direct_url,
        dash,
    })
}

static RE_REP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<Representation\b.*?</Representation>").unwrap());
static RE_REP_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<Representation\b[^>]*>").unwrap());
static RE_TEMPLATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<SegmentTemplate\b[^>]*>").unwrap());
static RE_TIMELINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<SegmentTimeline>(.*?)</SegmentTimeline>").unwrap());
static RE_SEG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<S\b[^>]*>").unwrap());
static RE_ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(\w+)="([^"]*)""#).unwrap());
static RE_DUR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"mediaPresentationDuration="PT(?:(\d+(?:\.\d+)?)M)?(?:(\d+(?:\.\d+)?)S)?"#)
        .unwrap()
});

// Parse the highest-bandwidth representation of a Tidal audio MPD. The
// segment template lives inside the representation; a whole-doc fallback
// covers a shared template above it.
fn parse_dash(mpd: &str) -> Option<DashInfo> {
    RE_REP
        .find_iter(mpd)
        .filter_map(|m| parse_rep(m.as_str(), mpd))
        .max_by_key(|d| d.bandwidth)
}

fn parse_rep(block: &str, mpd: &str) -> Option<DashInfo> {
    let open = RE_REP_OPEN.find(block).map(|m| m.as_str())?;
    let rep_attrs = tag_attrs(open);
    let template = RE_TEMPLATE
        .find(block)
        .or_else(|| RE_TEMPLATE.find(mpd))
        .map(|m| m.as_str())?;
    let tpl_attrs = tag_attrs(template);
    let segments = parse_segments(block);
    let (segments, timescale) = if segments.is_empty() {
        // No timeline: one segment of the full duration, in seconds.
        match duration_seconds(mpd) {
            Some(secs) => (vec![Segment { samples: secs, count: 1 }], 1),
            None => (segments, num_attr(&tpl_attrs, "timescale").unwrap_or(1)),
        }
    } else {
        (segments, num_attr(&tpl_attrs, "timescale").unwrap_or(1))
    };
    Some(DashInfo {
        bandwidth: num_attr(&rep_attrs, "bandwidth")?,
        init_url: str_attr(&tpl_attrs, "initialization")?,
        media_url: str_attr(&tpl_attrs, "media")?,
        timescale,
        start_number: num_attr(&tpl_attrs, "startNumber").unwrap_or(1),
        segments,
        codec: str_attr(&rep_attrs, "codecs").unwrap_or_default(),
        sample_rate: num_attr(&rep_attrs, "audioSamplingRate").unwrap_or(0),
        bit_depth: str_attr(&rep_attrs, "id")
            .and_then(|id| id.rsplit(',').next()?.parse().ok())
            .unwrap_or(0),
    })
}

// d="..." r="..." entries inside a SegmentTimeline.
fn parse_segments(block: &str) -> Vec<Segment> {
    let Some(tl) = RE_TIMELINE.captures(block) else {
        return Vec::new();
    };
    RE_SEG
        .find_iter(&tl[1])
        .filter_map(|m| {
            let attrs = tag_attrs(m.as_str());
            let d: u64 = num_attr(&attrs, "d")?;
            let r: u32 = num_attr(&attrs, "r").unwrap_or(0);
            (d > 0).then_some(Segment {
                samples: d,
                count: r + 1,
            })
        })
        .collect()
}

fn tag_attrs(tag: &str) -> Vec<(String, String)> {
    RE_ATTR
        .captures_iter(tag)
        .map(|c| (c[1].to_string(), xml_unescape(&c[2])))
        .collect()
}

// An MPD is XML, so '&' inside a URL attribute is written '&amp;'.
// Segment URLs keep the raw text otherwise, and a player fetching
// '...token=X&amp;info=Y' mangles the query string into 'amp;info=Y',
// which the CDN signature check rejects with 403. Decode the small
// entity set; '&amp;' last so '&amp;lt;' decodes to literal '&lt;'.
fn xml_unescape(s: &str) -> String {
    let mut out = s
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    if s.contains("&amp;") {
        out = out.replace("&amp;", "&");
    }
    out
}

fn str_attr(attrs: &[(String, String)], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

fn num_attr<T: FromStr>(attrs: &[(String, String)], name: &str) -> Option<T> {
    str_attr(attrs, name)?.parse().ok()
}

// PT3M30.373S -> 210 seconds. Only feeds the no-timeline fallback in
// parse_rep; Tidal audio MPDs always carry a timeline.
fn duration_seconds(mpd: &str) -> Option<u64> {
    let caps = RE_DUR.captures(mpd)?;
    let mins: f64 = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
    let secs: f64 = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
    Some((mins * 60.0 + secs) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real shape of a hi-res MPD (captured live), URLs shortened.
    const MPD: &str = r#"<?xml version='1.0' encoding='UTF-8'?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT3M30.373S"><Period><AdaptationSet contentType="audio" mimeType="audio/mp4"><Representation id="FLAC_HIRES,44100,24" codecs="flac" bandwidth="1616237" audioSamplingRate="44100"><SegmentTemplate timescale="44100" initialization="https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/0.mp4?token=T" media="https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/$Number$.mp4?token=T" startNumber="1"><SegmentTimeline><S d="176128" r="51"/><S d="118808"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;

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
    fn unescapes_xml_entities_in_segment_urls() {
        // A real v2 MPD escapes '&' inside its URL attributes as '&amp;'.
        // The served playlist must carry the decoded single '&', or the
        // player mangles the query string and the CDN answers 403.
        let mpd = r#"<MPD><Period><AdaptationSet mimeType="audio/mp4"><Representation id="AACLC,44100,16" codecs="mp4a.40.2" bandwidth="1234" audioSamplingRate="44100"><SegmentTemplate timescale="44100" initialization="https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/0.mp4?token=T&amp;info=UEx&amp;foo=1" media="https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/$Number$.mp4?token=T&amp;info=UExWQ&amp;bar=2" startNumber="1"><SegmentTimeline><S d="176128" r="2"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;
        let d = parse_dash(mpd).expect("parses");
        assert_eq!(
            d.init_url,
            "https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/0.mp4?token=T&info=UEx&foo=1"
        );
        assert!(!d.media_url.contains("&amp;"));
        assert!(d.media_url.contains("?token=T"));
        assert!(d.media_url.contains("&info=UExWQ&bar=2"));
    }

    #[test]
    fn unescapes_amp_last() {
        // '&amp;lt;' is a literal '&lt;' after decoding, never '<'.
        assert_eq!(xml_unescape("&amp;lt;"), "&lt;");
        assert_eq!(xml_unescape("a&amp;b"), "a&b");
        assert_eq!(xml_unescape("&lt;&amp;&quot;"), "<&\"");
    }

    #[test]
    fn parses_hires_dash() {
        let d = parse_dash(MPD).expect("parses");
        assert_eq!(d.bandwidth, 1616237);
        assert_eq!(d.codec, "flac");
        assert_eq!(d.sample_rate, 44100);
        assert_eq!(d.bit_depth, 24);
        assert_eq!(d.timescale, 44100);
        assert!(d.init_url.contains("0.mp4"));
        assert!(d.media_url.contains("$Number$"));
        // 52 + 1 segments.
        assert_eq!(d.segments.len(), 2);
        assert_eq!(d.segments[0].samples, 176128);
        assert_eq!(d.segments[0].count, 52);
        assert_eq!(d.segments[1].samples, 118808);
        assert_eq!(d.segments[1].count, 1);
    }

    #[test]
    fn picks_highest_bandwidth_representation() {
        let m = MPD.replace(
            "</Representation></AdaptationSet>",
            r#"<Representation id="FLAC_LOSSLESS,44100,16" codecs="flac" bandwidth="800000" audioSamplingRate="44100"><SegmentTemplate timescale="44100" initialization="https://sp-ad-fa.audio.tidal.com/mediatracks/BBB/0.mp4?token=T" media="https://sp-ad-fa.audio.tidal.com/mediatracks/BBB/$Number$.mp4?token=T" startNumber="1"><SegmentTimeline><S d="176128" r="51"/><S d="118808"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet>"#,
        );
        let d = parse_dash(&m).expect("parses");
        assert_eq!(d.bandwidth, 1616237);
        assert!(d.init_url.contains("AAA"));
    }

    #[test]
    fn bts_has_no_dash() {
        let body: Value = serde_json::json!({
            "manifestMimeType": "application/vnd.tidal.bts",
            "manifest": base64::engine::general_purpose::STANDARD.encode(r#"{"mimeType":"audio/mp4","codecs":"mp4a.40.2","encryptionType":"NONE","urls":["https://cdn/1.mp4?token=x"]}"#)
        });
        let info = parse_stream_v1(body).unwrap();
        assert_eq!(info.direct_url.as_deref(), Some("https://cdn/1.mp4?token=x"));
        assert!(info.dash.is_none());
    }

    #[test]
    fn parses_v2_manifest_document() {
        let mpd = MPD;
        let b64 = base64::engine::general_purpose::STANDARD.encode(mpd);
        let body: Value = serde_json::json!({
            "data": {
                "type": "trackManifests",
                "id": "7",
                "attributes": {
                    "uri": format!("data:application/dash+xml;base64,{b64}"),
                    "formats": ["HEAACV1", "AACLC", "FLAC", "FLAC_HIRES"],
                },
            },
        });
        let info = parse_manifest(body).unwrap();
        assert_eq!(info.mime_type, "application/dash+xml");
        assert!(info.direct_url.is_none());
        let dash = info.dash.expect("dash parsed");
        assert_eq!(dash.bandwidth, 1616237);
    }

    #[test]
    fn parses_v2_https_uri_as_direct_url() {
        let body: Value = serde_json::json!({
            "data": { "attributes": { "uri": "https://cdn/1.mp4?token=x" } }
        });
        let info = parse_manifest(body).unwrap();
        assert_eq!(info.direct_url.as_deref(), Some("https://cdn/1.mp4?token=x"));
        assert!(info.dash.is_none());
    }

    #[test]
    fn formats_follow_tier() {
        assert_eq!(audio_quality_to_formats("HI_RES"), "HEAACV1,AACLC,FLAC,FLAC_HIRES");
        assert_eq!(audio_quality_to_formats("LOSSLESS"), "HEAACV1,AACLC,FLAC");
        assert_eq!(audio_quality_to_formats("HIGH"), "HEAACV1,AACLC");
        assert_eq!(audio_quality_to_formats("LOW"), "HEAACV1");
    }

    #[test]
    fn window_allows_five_per_five_seconds() {
        let mut recent = VecDeque::new();
        let t0 = Instant::now();
        for i in 0..STREAM_WINDOW_MAX as u64 {
            assert!(
                window_allows(&mut recent, t0 + Duration::from_millis(i)),
                "start {i} must pass"
            );
        }
        // One more start inside the window is rejected.
        assert!(!window_allows(&mut recent, t0 + Duration::from_secs(4)));
        // Once the first start is older than the window, a new one passes.
        assert!(window_allows(
            &mut recent,
            t0 + Duration::from_secs(5) + Duration::from_millis(1)
        ));
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
    fn unrelated_errors_do_not_count_toward_throttle() {
        let limiter = StreamLimiter::new();
        for _ in 0..THROTTLE_TRIGGER * 2 {
            limiter.note(FetchOutcome::Other);
        }
        let state = limiter.state.lock().unwrap();
        assert!(state.cooldown_until.is_none());
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn in_flight_failures_do_not_extend_the_pause() {
        let limiter = StreamLimiter::new();
        for _ in 0..THROTTLE_TRIGGER {
            limiter.note(FetchOutcome::Throttled);
        }
        let before = limiter.state.lock().unwrap().cooldown_until;
        assert!(before.is_some());
        // A failure recorded during the pause must not re-arm it.
        limiter.note(FetchOutcome::Throttled);
        let after = limiter.state.lock().unwrap().cooldown_until;
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn acquire_waits_out_an_active_pause() {
        let limiter = StreamLimiter::new();
        {
            let mut state = limiter.state.lock().unwrap();
            state.cooldown_until = Some(Instant::now() + Duration::from_millis(100));
        }
        let start = Instant::now();
        let permit = limiter.acquire().await.expect("acquires after the pause");
        assert!(start.elapsed() >= Duration::from_millis(80));
        drop(permit);
    }
}
