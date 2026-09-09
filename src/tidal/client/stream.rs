// Stream resolution: the v1 playbackinfo endpoint. Backs both /stream
// and /download. Tidal answers with either a single whole-file URL (the
// lossy tiers) or a segmented DASH manifest (the FLAC tiers), whose
// segment URLs are extracted here and fetched by the handler.
//
// This module also holds the throttle that protects the account from a
// client bursting a whole queue of stream requests at once.
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::{StreamExt, stream};
use regex::Regex;
use serde_json::Value;
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::tidal::Quality;

use super::{API_URL, Error, TidalClient};

// The two single-file manifest shapes. BTS is the normal one; EMU is
// the same JSON without the encryption fields.
const BTS_MIME: &str = "application/vnd.tidal.bts";
const EMU_MIME: &str = "application/vnd.tidal.emu";
// Segmented MPEG-DASH. What Tidal returns for the FLAC tiers on a client
// entitled to them.
const DASH_MIME: &str = "application/dash+xml";

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

// One UUID v4 per stream fetch, sent as x-tidal-streamingsessionid.
// The official app generates one per playback session and correlates
// its analytics with it; playbackinfo expects the header present.
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

// What the client should be pointed at. Tidal answers with one of two
// shapes depending on the tier and the registered client: the lossy
// tiers come back as a single whole file, the FLAC tiers as a segmented
// DASH manifest.
#[derive(Clone, Debug)]
pub enum Asset {
    // A whole-file CDN URL (BTS/EMU). Served as a 302.
    File(String),
    // A segmented asset: an init header plus media segments that
    // concatenate into one fragmented MP4. Fetched and joined before
    // serving, because clients need a file rather than a manifest.
    Segmented { init: String, segments: Vec<String> },
}

// One playable asset plus what Tidal says it actually served. `quality`
// is the truth the request could only ask for, so callers log it rather
// than assuming the tier they requested came back.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub quality: Quality,
    // The codec as the manifest names it ("flac", "mp4a.40.2", "ec-3", …).
    pub codec: String,
    // Reported by playbackinfo. Absent on lossy tiers, where Tidal
    // sends null.
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub asset: Asset,
    // True when the manifest carried a non-empty keyId: the CDN bytes
    // are AES-128-CTR ciphertext and a bare redirect hands the client
    // undecodable audio. Callers must not serve this silently.
    pub encrypted: bool,
}

impl TidalClient {
    // Resolve a track to a playable CDN URL at (at most) the given tier.
    //
    // This is the legacy v1 playbackinfo endpoint, not the v2
    // trackManifests one. v2 cannot express Dolby Atmos at all (its
    // format list has no EAC3_JOC member and it reports every asset as
    // STEREO), and it returns no bit depth or sample rate, so there is
    // no way to tell from its response what was actually served. v1
    // answers with `audioMode`, `audioQuality`, `bitDepth` and
    // `sampleRate`, and its BTS manifest is one whole-file URL rather
    // than a segmented playlist.
    //
    // Never cached: the returned URL carries a short-lived signature.
    pub async fn stream_info(
        &self,
        track_id: u64,
        quality: Quality,
        mode: &str,
    ) -> Result<StreamInfo, Error> {
        // Throttle: wait (bounded) for a concurrency and window slot.
        // The permit stays held across the HTTP call.
        let _permit = self.stream_limiter.acquire().await?;
        let token = self.access_token().await?;
        // The account's country decides which assets are licensed to it;
        // omitting it narrows what the endpoint hands back.
        let cc = self.country_code().await?;
        let mut query = vec![
            ("audioquality", quality.as_audioquality()),
            ("playbackmode", mode),
            ("assetpresentation", "FULL"),
        ];
        if let Some(cc) = &cc {
            query.push(("countryCode", cc.as_str()));
        }
        let result = async {
            let resp = self
                .http
                // *postpaywall*, not the plain `playbackinfo` the web SDK
                // calls. The plain endpoint serves what an unsubscribed
                // session is entitled to and caps at AAC regardless of the
                // audioquality asked for; only the postpaywall variant
                // honours the subscription's lossless/hi-res entitlement.
                .get(format!("{API_URL}/tracks/{track_id}/playbackinfopostpaywall"))
                .bearer_auth(token)
                .header("x-tidal-client-version", super::CLIENT_VERSION)
                .header("X-Playback-Session-Id", new_session_id())
                .query(&query)
                .send()
                .await?;
            let status = resp.status();
            // Read the raw body first. resp.json() would discard the
            // text on a decode failure, but a throttled response is
            // HTML or empty, and that text is the diagnostic the
            // circuit breaker below keys on.
            let text = resp.text().await?;
            let body: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => return Err(Error::HttpDecode(status.as_u16(), text)),
            };
            if !status.is_success() {
                return Err(Error::Tidal(status.as_u16(), body.to_string()));
            }
            // Everything except the manifest itself, which is a large
            // base64 blob. A silent downgrade shows up here as the
            // response's own audioQuality/audioMode/assetPresentation.
            tracing::trace!(
                "playbackinfo {track_id} asked={} -> {}",
                quality.as_audioquality(),
                serde_json::to_string(&{
                    let mut b = body.clone();
                    if let Some(o) = b.as_object_mut() {
                        o.remove("manifest");
                    }
                    b
                })
                .unwrap_or_default()
            );
            parse_playback_info(body)
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

    // A download asks for the offline asset first, like the official
    // app. A mode rejection, for example no offline entitlement, falls
    // back to the streaming mode; a throttle-signature or queue-full
    // failure does not, because a retry under the same conditions
    // fails the same way.
    pub(crate) async fn download_info(
        &self,
        track_id: u64,
        quality: Quality,
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

// A hard ceiling on one assembled track. A long hi-res track is around
// a hundred megabytes; this bounds a single request against a manifest
// claiming far more than a track could hold.
const MAX_DOWNLOAD_BYTES: usize = 512 * 1024 * 1024;
// How many segments to fetch at once. Assembly is bound by CDN round
// trips and bandwidth, not CPU, so this is the main latency lever:
// 56 segments at 8-wide is seven round-trip waves, at 16-wide four.
// Kept well under the segment count so a track still does not arrive
// as one burst.
pub const SEGMENT_CONCURRENCY: usize = 16;
// Concurrency for the size sweep. Sized so a typical track (60-90
// segments) measures in one or two round-trip waves instead of five.
const HEAD_CONCURRENCY: usize = 48;

impl TidalClient {
    // The byte length of every part, in order, via HEAD.
    //
    // This is what makes progressive serving possible: knowing the sizes
    // up front gives an exact Content-Length without downloading
    // anything, so the body can start flowing at the first segment while
    // seeking still works. It also makes a range request cheap, since
    // the offsets say which segments it actually needs.
    //
    // None when any part does not report a usable length — the caller
    // falls back to buffering the whole track, which needs no sizes.
    pub(crate) async fn segment_sizes(&self, urls: Vec<String>) -> Option<Vec<u64>> {
        let sizes: Vec<Option<u64>> = stream::iter(urls)
            .map(|url| async move {
                let resp = self.http.head(&url).send().await.ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                resp.content_length()
            })
            // Far wider than the segment fetches. This sweep sits in
            // front of the first byte of audio, and every wave of it is
            // pure latency the listener waits through — but a HEAD
            // carries no body, so going wide costs round trips, not
            // bandwidth, and none of the throttle-avoidance reasoning
            // that caps segment fetches applies.
            .buffered(HEAD_CONCURRENCY)
            .collect()
            .await;
        // All or nothing: a partial table would give a wrong
        // Content-Length, which is worse than not streaming progressively.
        sizes.into_iter().collect()
    }

    // The first `len` bytes of each part, in order.
    //
    // Used to read segment box headers without downloading the audio
    // behind them: a fragment's sizes live in its first few hundred
    // bytes, so a small prefix answers "how long is this really" for a
    // whole track in one wave of requests.
    //
    // None for any part the CDN will not range-serve, which the caller
    // treats as "cannot size cheaply".
    pub(crate) async fn fetch_prefixes(
        &self,
        urls: Vec<String>,
        len: u64,
    ) -> Vec<Option<bytes::Bytes>> {
        stream::iter(urls)
            .map(|url| async move {
                let resp = self
                    .http
                    .get(&url)
                    .header("Range", format!("bytes=0-{}", len.saturating_sub(1)))
                    .send()
                    .await
                    .ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                resp.bytes().await.ok()
            })
            .buffered(HEAD_CONCURRENCY)
            .collect()
            .await
    }

    // Fetch one part, returning its bytes.
    pub(crate) async fn fetch_one(&self, url: &str) -> Result<bytes::Bytes, Error> {
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), "segment fetch failed".into()));
        }
        Ok(resp.bytes().await?)
    }

    // Fetch an init segment plus its media segments and concatenate them
    // into one fragmented-MP4 body, for a caller that must buffer the
    // whole track. Used when the sizes could not be measured cheaply, so
    // progressive serving is off the table.
    //
    // Fetched with bounded concurrency: sequential would put a
    // multi-second stall in front of every play, while unbounded would
    // open one connection per segment, which is exactly the burst the
    // stream limiter exists to prevent. `buffered` preserves order, so
    // the concatenation stays correct regardless of completion order.
    pub(crate) async fn fetch_segments(
        &self,
        init: String,
        segments: Vec<String>,
    ) -> Result<Vec<u8>, Error> {
        let urls: Vec<String> = std::iter::once(init).chain(segments).collect();
        let total = urls.len();
        let parts: Vec<Result<bytes::Bytes, Error>> = stream::iter(urls.into_iter().enumerate())
            .map(|(i, url)| async move {
                let resp = self.http.get(&url).send().await?;
                let status = resp.status();
                if !status.is_success() {
                    // Name the segment: a mid-track failure is usually
                    // an expired token, which looks nothing like a
                    // failure on the first one.
                    return Err(Error::Tidal(
                        status.as_u16(),
                        format!("segment {i} of {total} failed"),
                    ));
                }
                Ok(resp.bytes().await?)
            })
            .buffered(SEGMENT_CONCURRENCY)
            .collect()
            .await;

        let mut out: Vec<u8> = Vec::new();
        for part in parts {
            let bytes = part?;
            if out.len() + bytes.len() > MAX_DOWNLOAD_BYTES {
                return Err(Error::Auth(format!(
                    "assembled track exceeds {MAX_DOWNLOAD_BYTES} bytes; refusing to buffer it"
                )));
            }
            out.extend_from_slice(&bytes);
        }
        Ok(out)
    }
}

// The tier Tidal says it served, from the response's own fields. The
// `audioMode` is what makes Atmos detectable: `audioQuality` never says
// Atmos, because Atmos is a presentation of a LOSSLESS-quality asset
// rather than a quality of its own.
fn served_quality(body: &Value) -> Quality {
    if body["audioMode"].as_str() == Some("DOLBY_ATMOS") {
        return Quality::Atmos;
    }
    match body["audioQuality"].as_str() {
        Some("HI_RES_LOSSLESS") | Some("HI_RES") => Quality::HiRes,
        Some("LOSSLESS") => Quality::Lossless,
        Some("LOW") => Quality::Low,
        // HIGH, and anything unrecognized: the lossy tier is the safe
        // reading, since every tier above it is named explicitly.
        _ => Quality::High,
    }
}

// Decode a v1 playbackinfo response. `manifest` is base64; its shape is
// named by `manifestMimeType`. Two shapes arrive in practice:
//
//   * BTS/EMU — JSON with a `urls` array whose first entry is the whole
//     file. The lossy tiers, and what a redirect can serve directly.
//   * DASH — a segmented MPD. What Tidal returns for FLAC on a client
//     entitled to it. Converted to an HLS playlist here rather than
//     refused, because its segment URLs are usable as-is.
fn parse_playback_info(body: Value) -> Result<StreamInfo, Error> {
    let mime = body["manifestMimeType"].as_str().unwrap_or_default();
    let raw = body["manifest"]
        .as_str()
        .ok_or_else(|| Error::Auth("response missing manifest".into()))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw))
        .map_err(|e| Error::Auth(format!("manifest decode failed: {e}")))?;

    let quality = served_quality(&body);
    // The API sends null on tiers where these do not apply.
    let sample_rate = body["sampleRate"].as_u64().map(|v| v as u32);
    let bit_depth = body["bitDepth"].as_u64().map(|v| v as u32);

    if mime == DASH_MIME {
        let mpd = String::from_utf8_lossy(&decoded);
        let dash = parse_dash(&mpd)
            .ok_or_else(|| Error::Auth("dash manifest carries no playable segments".into()))?;
        return Ok(StreamInfo {
            quality,
            codec: dash.codec,
            sample_rate: sample_rate.or(dash.sample_rate),
            bit_depth: bit_depth.or(dash.bit_depth),
            asset: Asset::Segmented {
                init: dash.init,
                segments: dash.segments,
            },
            // Tidal's DASH audio carries no ContentProtection; the
            // segments are in the clear.
            encrypted: false,
        });
    }

    if mime != BTS_MIME && mime != EMU_MIME {
        return Err(Error::Auth(format!(
            "unsupported manifest type {mime:?}; expected BTS, EMU or DASH"
        )));
    }
    let manifest: Value = serde_json::from_slice(&decoded)
        .map_err(|e| Error::Auth(format!("manifest is not JSON: {e}")))?;
    let url = manifest["urls"][0]
        .as_str()
        .ok_or_else(|| Error::Auth("manifest carries no stream url".into()))?
        .to_string();
    // An empty keyId means the asset is served in the clear. EMU
    // manifests carry no keyId field at all, which reads the same way.
    let encrypted = manifest["keyId"].as_str().is_some_and(|k| !k.is_empty());
    Ok(StreamInfo {
        quality,
        codec: manifest["codecs"].as_str().unwrap_or_default().to_string(),
        sample_rate,
        bit_depth,
        asset: Asset::File(url),
        encrypted,
    })
}

// What one DASH Representation yields.
struct Dash {
    codec: String,
    sample_rate: Option<u32>,
    bit_depth: Option<u32>,
    init: String,
    segments: Vec<String>,
}

static RE_SEGMENT_TEMPLATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<SegmentTemplate\b([^>]*)>(.*?)</SegmentTemplate>").unwrap());
static RE_REPRESENTATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<Representation\b([^>]*)>").unwrap());
static RE_S: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<S\b([^>]*)/?>").unwrap());

fn attr(attrs: &str, name: &str) -> Option<String> {
    // Attributes in these captures are space-delimited key="value" pairs.
    // Avoid compiling a regex per lookup; this runs in the segment loop.
    let needle = format!(" {name}=\"");
    let start = if let Some(p) = attrs.find(&needle) {
        p + needle.len()
    } else {
        let needle0 = format!("{name}=\"");
        if attrs.starts_with(&needle0) {
            needle0.len()
        } else {
            return None;
        }
    };
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// A hostile or malformed manifest must not be able to make this
// allocate without bound. A track is minutes long at a few seconds per
// segment; five figures is already far past anything real.
const MAX_SEGMENTS: usize = 50_000;

// Convert a Tidal DASH manifest into the list of segments it names.
//
// The segment URLs are Tidal's own CDN URLs with their signed tokens
// intact; the server fetches them here and rewraps the bytes, so the
// tokens are never exposed to the client.
//
// Only the first Representation is read. Tidal sends exactly one for
// audio (`adaptive=false` behaviour); a second would be an alternative
// bitrate that a Subsonic client has no way to choose between anyway.
fn parse_dash(mpd: &str) -> Option<Dash> {
    let rep = RE_REPRESENTATION.captures(mpd)?;
    let rep_attrs = rep[1].to_string();
    let codec = attr(&rep_attrs, "codecs").unwrap_or_default();
    let sample_rate = attr(&rep_attrs, "audioSamplingRate").and_then(|v| v.parse().ok());
    // The Representation id encodes the format triple, e.g.
    // id="FLAC,44100,16" — the only place a DASH manifest carries the
    // bit depth. Lossy ids are a bare name ("AACLC") with no triple, so
    // a missing one is normal rather than an error.
    let id = attr(&rep_attrs, "id").unwrap_or_default();
    let bit_depth = match id.split(',').collect::<Vec<_>>().as_slice() {
        [_format, _rate, d] => d.parse::<u32>().ok(),
        _ => None,
    };

    let tpl = RE_SEGMENT_TEMPLATE.captures(mpd)?;
    let (tpl_attrs, timeline) = (tpl[1].to_string(), tpl[2].to_string());
    let init = attr(&tpl_attrs, "initialization")?;
    let media = attr(&tpl_attrs, "media")?;
    // Per the DASH spec this defaults to 1 when absent.
    let start: u64 = attr(&tpl_attrs, "startNumber")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    // <S d="..." r="..."/> — `r` is the number of *additional* repeats,
    // so r="54" means 55 segments. Only the count matters here: the
    // durations exist for playlist timing, and nothing downstream needs
    // them now that tracks are served as one file.
    let mut count: usize = 0;
    for s in RE_S.captures_iter(&timeline) {
        let attrs = &s[1];
        if attr(attrs, "d").is_none() {
            continue;
        }
        let repeats: i64 = attr(attrs, "r")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
            // A negative r means "until the period ends", which needs a
            // duration this manifest does not reliably carry. Treat it
            // as a single segment rather than guessing a count.
            .max(0);
        count = count.saturating_add(repeats as usize + 1).min(MAX_SEGMENTS);
        if count >= MAX_SEGMENTS {
            break;
        }
    }
    // No segments means no audio. Returning an empty list would produce
    // a file containing only the init header, which clients report as a
    // corrupt track rather than an error.
    if count == 0 {
        return None;
    }

    let segments = (0..count)
        .map(|i| substitute_number(&media, start + i as u64))
        .collect();

    Some(Dash {
        codec,
        sample_rate,
        bit_depth,
        init,
        segments,
    })
}

// Expand `$Number$` (and its zero-padded `$Number%0Nd$` form) in a DASH
// media template. `$$` is the spec's escape for a literal dollar and is
// unescaped last so it cannot be mistaken for an identifier.
fn substitute_number(template: &str, n: u64) -> String {
    static RE_NUMBER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$Number(?:%0(\d+)d)?\$").unwrap());
    let out = RE_NUMBER.replace_all(template, |c: &regex::Captures| {
        match c.get(1).and_then(|w| w.as_str().parse::<usize>().ok()) {
            Some(width) => format!("{n:0width$}"),
            None => n.to_string(),
        }
    });
    out.replace("$$", "$")
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A BTS manifest as the API serves it (URL shortened), wrapped in a
    // playbackinfo response.
    fn playback_info(manifest: Value, extra: Value) -> Value {
        let b64 = base64::engine::general_purpose::STANDARD.encode(manifest.to_string());
        let mut body = json!({
            "trackId": 7,
            "manifestMimeType": BTS_MIME,
            "manifest": b64,
        });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        body
    }

    #[test]
    fn parses_a_lossless_bts_manifest() {
        let body = playback_info(
            json!({
                "mimeType": "audio/flac",
                "codecs": "flac",
                "encryptionType": "NONE",
                "keyId": "",
                "urls": ["https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/0.flac?token=T"],
            }),
            json!({"audioQuality": "LOSSLESS", "audioMode": "STEREO", "bitDepth": 16, "sampleRate": 44100}),
        );
        let info = parse_playback_info(body).unwrap();
        assert_eq!(info.quality, Quality::Lossless);
        assert_eq!(info.codec, "flac");
        assert_eq!(info.bit_depth, Some(16));
        assert_eq!(info.sample_rate, Some(44100));
        match &info.asset {
            Asset::File(u) => assert!(u.starts_with("https://sp-ad-fa.audio.tidal.com/")),
            Asset::Segmented { .. } => panic!("a BTS manifest is a single file, not segments"),
        }
        // An empty keyId means the bytes are in the clear, so a plain
        // redirect is safe.
        assert!(!info.encrypted);
    }

    #[test]
    fn atmos_is_detected_from_audio_mode_not_quality() {
        // audioQuality reads LOSSLESS on an Atmos asset; only audioMode
        // distinguishes it. Reading quality alone would report stereo
        // FLAC for a 6-channel EAC3 stream.
        let body = playback_info(
            json!({"codecs": "ec-3", "keyId": "", "urls": ["https://cdn/a.mp4"]}),
            json!({"audioQuality": "LOSSLESS", "audioMode": "DOLBY_ATMOS"}),
        );
        let info = parse_playback_info(body).unwrap();
        assert_eq!(info.quality, Quality::Atmos);
        assert_eq!(info.codec, "ec-3");
    }

    #[test]
    fn hi_res_reports_its_real_depth_and_rate() {
        let body = playback_info(
            json!({"codecs": "flac", "keyId": "", "urls": ["https://cdn/a.flac"]}),
            json!({"audioQuality": "HI_RES_LOSSLESS", "audioMode": "STEREO", "bitDepth": 24, "sampleRate": 96000}),
        );
        let info = parse_playback_info(body).unwrap();
        assert_eq!(info.quality, Quality::HiRes);
        assert_eq!(info.bit_depth, Some(24));
        assert_eq!(info.sample_rate, Some(96_000));
    }

    #[test]
    fn lossy_tiers_send_null_depth_and_rate() {
        // The API sends JSON null rather than omitting the fields.
        let body = playback_info(
            json!({"codecs": "mp4a.40.2", "keyId": "", "urls": ["https://cdn/a.mp4"]}),
            json!({"audioQuality": "HIGH", "audioMode": "STEREO", "bitDepth": null, "sampleRate": null}),
        );
        let info = parse_playback_info(body).unwrap();
        assert_eq!(info.quality, Quality::High);
        assert_eq!(info.bit_depth, None);
        assert_eq!(info.sample_rate, None);
    }

    #[test]
    fn a_non_empty_key_id_marks_the_stream_encrypted() {
        let body = playback_info(
            json!({"codecs": "flac", "keyId": "abc123", "urls": ["https://cdn/a.flac"]}),
            json!({"audioQuality": "LOSSLESS"}),
        );
        assert!(parse_playback_info(body).unwrap().encrypted);
    }

    #[test]
    fn emu_manifests_carry_no_key_id_and_read_as_clear() {
        let b64 = base64::engine::general_purpose::STANDARD
            .encode(json!({"mimeType": "audio/flac", "urls": ["https://cdn/a.flac"]}).to_string());
        let body = json!({
            "manifestMimeType": EMU_MIME,
            "manifest": b64,
            "audioQuality": "LOSSLESS",
        });
        let info = parse_playback_info(body).unwrap();
        assert!(!info.encrypted);
        assert_eq!(info.codec, "");
    }

    #[test]
    fn unhandled_manifest_types_are_rejected() {
        // DASH is converted to HLS (see dash_tests); anything else has
        // no representation here and must fail loudly rather than be
        // handed to the client half-understood.
        let body = json!({
            "manifestMimeType": "application/vnd.apple.mpegurl",
            "manifest": "AAAA",
        });
        let err = parse_playback_info(body).unwrap_err();
        assert!(err.to_string().contains("unsupported manifest type"));
    }

    #[test]
    fn a_dash_manifest_that_does_not_parse_is_an_error_not_a_silent_empty() {
        let body = json!({
            "manifestMimeType": DASH_MIME,
            "manifest": base64::engine::general_purpose::STANDARD.encode("<MPD></MPD>"),
        });
        let err = parse_playback_info(body).unwrap_err();
        assert!(err.to_string().contains("no playable segments"));
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

#[cfg(test)]
mod dash_tests {
    use super::*;
    use serde_json::json;

    // The shape Tidal actually returns for a FLAC track (captured live,
    // URLs and tokens shortened). One Representation, a SegmentTemplate
    // with $Number$, and a SegmentTimeline whose `r` compresses the
    // repeated middle segments.
    const MPD: &str = r#"<?xml version='1.0' encoding='UTF-8'?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" minBufferTime="PT3.993S" mediaPresentationDuration="PT3M42.013S"><Period id="0"><AdaptationSet id="0" contentType="audio" mimeType="audio/mp4" lang="und"><Role schemeIdUri="urn:mpeg:dash:role:2011" value="main"/><Representation id="FLAC,44100,16" codecs="flac" bandwidth="863212" audioSamplingRate="44100"><AudioChannelConfiguration schemeIdUri="urn:mpeg:dash:23003:3:audio_channel_configuration:2011" value="2"/><SegmentTemplate timescale="44100" initialization="https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/0.mp4?token=T" media="https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/$Number$.mp4?token=T" startNumber="1"><SegmentTimeline><S d="176128" r="54"/><S d="103748"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;

    #[test]
    fn dash_yields_the_init_header_and_every_segment_url() {
        let d = parse_dash(MPD).expect("parses");
        assert_eq!(d.codec, "flac");
        assert_eq!(d.sample_rate, Some(44100));
        // The Representation id triple is the only source of bit depth
        // in a DASH manifest.
        assert_eq!(d.bit_depth, Some(16));
        assert_eq!(d.init, "https://sp-ad-fa.audio.tidal.com/mediatracks/AAA/0.mp4?token=T");
        // The signed token must survive into every segment URL, or the
        // fetch gets 403s instead of audio.
        assert!(d.segments.iter().all(|s| s.ends_with("?token=T")));
        // $Number$ must be fully expanded; a literal left behind would
        // be requested verbatim.
        assert!(!d.segments.iter().any(|s| s.contains("$Number$")));
    }

    #[test]
    fn the_repeat_count_is_additional_segments_not_total() {
        // r="54" is 55 segments, plus the trailing single = 56. Reading
        // `r` as a total would truncate the track by a segment, which
        // sounds like playback simply ending early.
        let d = parse_dash(MPD).expect("parses");
        assert_eq!(d.segments.len(), 56);
        assert!(d.segments[0].ends_with("/1.mp4?token=T"));
        assert!(d.segments[54].ends_with("/55.mp4?token=T"));
        assert!(d.segments[55].ends_with("/56.mp4?token=T"));
        // The init header is not one of the media segments; including it
        // twice would corrupt the concatenation.
        assert!(!d.segments.iter().any(|s| s == &d.init));
    }

    #[test]
    fn a_start_number_other_than_one_is_honored() {
        let m = MPD.replace(r#"startNumber="1""#, r#"startNumber="7""#);
        let d = parse_dash(&m).expect("parses");
        assert!(d.segments[0].ends_with("/7.mp4?token=T"));
        assert_eq!(d.segments.len(), 56);
    }

    #[test]
    fn a_lossy_representation_id_carries_no_bit_depth() {
        // Lossy ids are a bare name with no triple. Inventing a depth
        // would be worse than reporting none.
        let m = MPD.replace(r#"id="FLAC,44100,16""#, r#"id="AACLC""#);
        let d = parse_dash(&m).expect("parses");
        assert_eq!(d.bit_depth, None);
        assert_eq!(d.segments.len(), 56);
    }

    #[test]
    fn zero_padded_number_templates_expand() {
        assert_eq!(substitute_number("a/$Number%05d$.mp4", 42), "a/00042.mp4");
        assert_eq!(substitute_number("a/$Number$.mp4", 42), "a/42.mp4");
        // `$$` is the spec's literal-dollar escape.
        assert_eq!(substitute_number("a$$b/$Number$", 1), "a$b/1");
    }

    #[test]
    fn a_manifest_without_segments_is_refused() {
        // An empty timeline would assemble to just the init header,
        // which clients report as a corrupt track rather than an error.
        let m = MPD.replace(r#"<S d="176128" r="54"/><S d="103748"/>"#, "");
        assert!(parse_dash(&m).is_none());
        assert!(parse_dash("<MPD></MPD>").is_none());
    }

    #[test]
    fn parse_playback_info_routes_dash_to_segments_and_bts_to_a_file() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(MPD);
        let body = json!({
            "manifestMimeType": DASH_MIME,
            "manifest": b64,
            "audioQuality": "LOSSLESS",
            "audioMode": "STEREO",
            "bitDepth": 16,
            "sampleRate": 44100,
        });
        let info = parse_playback_info(body).unwrap();
        assert_eq!(info.quality, Quality::Lossless);
        assert_eq!(info.codec, "flac");
        assert_eq!(info.bit_depth, Some(16));
        // Tidal's DASH audio carries no ContentProtection.
        assert!(!info.encrypted);
        match info.asset {
            Asset::Segmented { init, segments } => {
                assert!(init.ends_with("/0.mp4?token=T"));
                assert_eq!(segments.len(), 56);
            }
            Asset::File(u) => panic!("segmented manifest must not become a file url: {u}"),
        }
    }

    #[test]
    fn an_unknown_manifest_type_is_still_refused() {
        let body = json!({
            "manifestMimeType": "application/vnd.apple.mpegurl",
            "manifest": base64::engine::general_purpose::STANDARD.encode("#EXTM3U"),
        });
        let err = parse_playback_info(body).unwrap_err();
        assert!(err.to_string().contains("unsupported manifest type"));
    }
}
