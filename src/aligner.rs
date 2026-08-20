// Forced-alignment sidecar client. When an aligner_url is configured and
// the client asks for enhanced lyrics, the getLyricsBySongId handler posts
// the track's Tidal CDN URL here and maps the word timestamps into
// version-2 cueLine data.
//
// The sidecar is a separate FastAPI process (aligner/ in this repo) that
// wraps Qwen3ForcedAligner. It downloads and decodes the audio itself, so
// this module stays free of audio/decoding dependencies.
//
// Callers treat a failure here as "no enhanced data" and fall back to the
// version-1 reply. The aligner never fails a lyric request.

use moka::future::Cache;
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::Duration;

// One timed word from the aligner. charStart/charEnd are 0-based inclusive
// char offsets into the line text the client submitted.
#[derive(Clone, Deserialize)]
pub struct AlignedWord {
    #[serde(rename = "charStart")]
    pub char_start: usize,
    #[serde(rename = "charEnd")]
    pub char_end: usize,
    #[serde(rename = "startTime")]
    pub start_time: f64,
    #[serde(rename = "endTime")]
    #[allow(dead_code)] // part of the sidecar contract; not used to build cues
    pub end_time: f64,
    pub text: String,
}

// One aligned line, keyed by its index in the submitted line list.
#[derive(Clone, Deserialize)]
pub struct AlignedLine {
    pub index: usize,
    pub value: String,
    pub words: Vec<AlignedWord>,
}

#[derive(Deserialize)]
struct AlignResponse {
    lines: Vec<AlignedLine>,
}

// Cache of alignment results per track. The cache key is the track id and
// the line payload hash: a lyrics edit changes the key and forces a refetch.
static CACHE: OnceLock<Cache<String, Vec<AlignedLine>>> = OnceLock::new();

fn cache() -> &'static Cache<String, Vec<AlignedLine>> {
    CACHE.get_or_init(|| {
        let ttl = crate::SETTINGS
            .get()
            .map(|s| s.aligner_ttl)
            .unwrap_or(6 * 3600);
        Cache::builder()
            .time_to_live(Duration::from_secs(ttl))
            .max_capacity(2_000)
            .build()
    })
}

pub struct Aligner {
    url: String,
    http: reqwest::Client,
}

// The aligner singleton, built lazily from settings. None when no
// aligner_url is configured, which disables enhanced word timing.
static ALIGNER: OnceLock<Option<Aligner>> = OnceLock::new();

pub fn get() -> Option<&'static Aligner> {
    ALIGNER
        .get_or_init(|| {
            crate::SETTINGS
                .get()
                .and_then(|s| s.aligner_url.clone())
                .map(Aligner::new)
        })
        .as_ref()
}

impl Aligner {
    pub fn new(url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .expect("failed to build aligner client");
        Self { url, http }
    }

    // Request word alignment for a track. Returns the aligned lines, or
    // None when the track is out of scope (too long, unsupported) or the
    // sidecar fails. The caller falls back to version-1 on None.
    pub async fn align(
        &self,
        track_id: u64,
        audio_url: &str,
        language: &str,
        lines: &[String],
    ) -> Option<Vec<AlignedLine>> {
        let key = align_key(track_id, lines);
        if let Some(hit) = cache().get(&key).await {
            return Some(hit);
        }
        let body = serde_json::json!({
            "audio_url": audio_url,
            "language": language,
            "text": lines,
        });
        let resp = match self.http.post(&self.url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("aligner request failed: {e}");
                return None;
            }
        };
        if !resp.status().is_success() {
            tracing::warn!("aligner returned {}", resp.status());
            return None;
        }
        let parsed: AlignResponse = match resp.json().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("aligner reply parse failed: {e}");
                return None;
            }
        };
        cache().insert(key, parsed.lines.clone()).await;
        Some(parsed.lines)
    }
}

// Deterministic cache key: track id + a hash of the joined lines. Lines
// differ across songs and across lyric edits, so both go into the key.
fn align_key(track_id: u64, lines: &[String]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    lines.hash(&mut h);
    format!("{track_id}:{:x}", h.finish())
}
