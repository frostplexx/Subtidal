// Stream metadata: playbackinfo manifest decoding. Backs the stream
// endpoint, which 302-redirects to a single-file Tidal CDN URL, or, for
// hi-res FLAC, rewrites the segmented DASH manifest into an HLS playlist.
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use regex::Regex;
use serde_json::Value;
use tokio::sync::{Semaphore, SemaphorePermit};

use super::{Error, TidalClient, API_URL};

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
const STREAM_LIMIT: usize = 6;
const STREAM_WINDOW: Duration = Duration::from_secs(5);
const STREAM_WINDOW_MAX: usize = 12;
// Bounded wait for a slot. Large enough that a downloader's whole
// queue passes (12 starts per 5 s drains ~140 tracks in a minute);
// the bound trips only on absurd bursts.
const STREAM_WAIT: Duration = Duration::from_secs(60);

pub(crate) struct StreamLimiter {
    semaphore: Semaphore,
    recent: Mutex<VecDeque<Instant>>,
}

impl StreamLimiter {
    pub(crate) fn new() -> Self {
        Self {
            semaphore: Semaphore::new(STREAM_LIMIT),
            recent: Mutex::new(VecDeque::new()),
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
            // The guard must end before the sleep below; scope it.
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

impl TidalClient {
    // Fetch playbackinfo for one track at the given quality tier.
    // Never cached: the CDN URLs carry short-lived signed tokens.
    pub async fn stream_info(&self, track_id: u64, quality: &str) -> Result<StreamInfo, Error> {
        // Throttle: wait (bounded) for a concurrency and window slot.
        // The permit stays held across the HTTP call.
        let _permit = self.stream_limiter.acquire().await?;
        let token = self.access_token().await?;
        let cc = self.country_code().await?;
        let mut query = vec![
            ("audioquality", quality),
            ("playbackmode", "STREAM"),
            ("assetpresentation", "FULL"),
        ];
        if let Some(cc) = &cc {
            query.push(("countryCode", cc.as_str()));
        }
        let resp = self
            .http
            .get(format!("{API_URL}/tracks/{track_id}/playbackinfopostpaywall"))
            .bearer_auth(token)
            .query(&query)
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await?;
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), body.to_string()));
        }
        parse_stream(body)
    }
}

// Decode a playbackinfo manifest into StreamInfo.
fn parse_stream(body: Value) -> Result<StreamInfo, Error> {
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
        // BTS: JSON with a direct single-file URL.
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
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
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
        let info = parse_stream(body).unwrap();
        assert_eq!(info.direct_url.as_deref(), Some("https://cdn/1.mp4?token=x"));
        assert!(info.dash.is_none());
    }

    #[test]
    fn window_allows_six_per_five_seconds() {
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
}
