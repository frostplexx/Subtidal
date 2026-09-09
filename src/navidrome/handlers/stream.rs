// Streaming and downloading, both resolved through Tidal's v1
// playbackinfo endpoint.
//
// Both endpoints answer with a file. A whole-file asset (the lossy
// tiers) becomes a 302 to Tidal's CDN and costs nothing here. A
// segmented one (the FLAC tiers) is fetched here and rewrapped into a
// native FLAC stream.
//
// Two earlier shapes did not work, and the reasons are worth keeping:
//
//   * An HLS playlist kept audio off this server entirely, but clients
//     use /stream to *download* as well as to play, and a playlist is
//     not a file — a downloader that saved one got a few KB of text
//     pointing at URLs that expire within the hour. Nothing in the
//     request distinguishes the two cases. (The segment URLs now carry
//     signed tokens, so this server fetches and rewraps them instead.)
//   * Concatenated fragmented MP4 is a file, but not one a player can
//     start on: a DASH init segment carries no duration and there is no
//     segment index, so the player downloaded and scanned the entire
//     track before decoding a frame. Delivering those bytes faster
//     could never fix it; the container was the problem.
//
// Native FLAC has neither flaw. It is a file, and it decodes from byte
// zero.
//
// The tier asked for is the client's own hint, else the configured
// default. Tidal decides what it will actually serve and downgrades
// silently, so the log line reports requested and served side by side.
use crate::navidrome::ids;
use crate::navidrome::params::QueryParams;
use crate::tidal::Quality;
use crate::tidal::client::{Asset, Error, SEGMENT_CONCURRENCY, StreamInfo};

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use moka::sync::Cache;

use super::{fail, redirect};
use warp::Reply;

// Manifest cache. Its job is not to save a round-trip but to keep
// repeat requests off the StreamLimiter: a client re-requests the same
// track on every pause/resume and after a failed load, and each of
// those otherwise consumes one of the 5-per-10s manifest slots, so a
// user skipping through a queue queues up multi-second waits behind
// requests for tracks they already fetched.
//
// The TTL is well inside the CDN signature's lifetime, so cached URLs
// are still fetchable; an expired entry just costs a fresh fetch.
const MANIFEST_TTL: Duration = Duration::from_secs(120);
static MANIFEST_CACHE: LazyLock<Cache<(u64, Quality), StreamInfo>> = LazyLock::new(|| {
    Cache::builder()
        .time_to_live(MANIFEST_TTL)
        .max_capacity(10_000)
        .build()
});

fn cached_manifest(track_id: u64, tier: Quality) -> Option<StreamInfo> {
    MANIFEST_CACHE.get(&(track_id, tier))
}

fn store_manifest(track_id: u64, tier: Quality, info: &StreamInfo) {
    MANIFEST_CACHE.insert((track_id, tier), info.clone());
}

// Assembled-audio cache. Concatenating a track costs dozens of CDN
// fetches, and clients re-request constantly — on pause/resume, on
// seek, and once per queue reshuffle. Without this every one of those
// re-downloads the whole track.
//
// Bounded in both directions: entries expire, and the total is capped
// by weight so a long queue cannot grow this without limit.
// Deliberately in memory and lost on restart — it is a latency cache,
// not storage. An entry that alone exceeds the byte budget is evicted
// rather than pinned, so one oversized track is re-fetched per request
// instead of holding the cache over its cap.
const AUDIO_TTL: Duration = Duration::from_secs(300);
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
static AUDIO_CACHE: LazyLock<Cache<(u64, Quality), Arc<Vec<u8>>>> = LazyLock::new(|| {
    Cache::builder()
        .time_to_live(AUDIO_TTL)
        .max_capacity(MAX_CACHE_BYTES)
        .weigher(|_, bytes: &Arc<Vec<u8>>| bytes.len() as u32)
        .build()
});

fn cached_audio(track_id: u64, tier: Quality) -> Option<Arc<Vec<u8>>> {
    AUDIO_CACHE.get(&(track_id, tier))
}

fn store_audio(track_id: u64, tier: Quality, bytes: &Arc<Vec<u8>>) {
    AUDIO_CACHE.insert((track_id, tier), Arc::clone(bytes));
}

// Per-track segment size tables, keyed like the audio cache. Cheap to
// hold (a few hundred integers) and worth keeping for the whole session:
// the sizes are a property of the asset, not of the signed URLs, so they
// stay valid even after the manifest is refetched.
static SIZE_CACHE: LazyLock<Cache<(u64, Quality), Arc<Vec<u64>>>> =
    LazyLock::new(|| Cache::builder().max_capacity(10_000).build());

// Segment cache, keyed by URL.
//
// This is the one that matters for playback. A player does not fetch a
// track once: it probes, reads the whole thing, re-reads the tail for
// the index, and issues fresh ranges on every seek. Those requests
// overlap heavily, so without a segment cache the same bytes are pulled
// from Tidal again and again — which is what made playback slow even
// after the response headers got fast.
//
// Keyed by URL rather than (track, tier) because that is the identity
// of the bytes, and it means a range request warms exactly the parts a
// later request will reuse.
const SEGMENT_TTL: Duration = Duration::from_secs(300);
const MAX_SEGMENT_CACHE_BYTES: u64 = 256 * 1024 * 1024;
static SEGMENT_CACHE: LazyLock<Cache<String, bytes::Bytes>> = LazyLock::new(|| {
    Cache::builder()
        .time_to_live(SEGMENT_TTL)
        .max_capacity(MAX_SEGMENT_CACHE_BYTES)
        .weigher(|_, part: &bytes::Bytes| part.len() as u32)
        .build()
});

// Fetch one part, serving it from the segment cache when warm.
async fn fetch_part(url: &str) -> Result<bytes::Bytes, Error> {
    if let Some(bytes) = SEGMENT_CACHE.get(url) {
        return Ok(bytes);
    }
    let bytes = crate::tidal::client().fetch_one(url).await?;
    // A single part larger than the whole budget is simply not held;
    // memory stays bounded either way.
    SEGMENT_CACHE.insert(url.to_string(), bytes.clone());
    Ok(bytes)
}

// One part of a progressive response: where its bytes come from and the
// slice of it that falls inside the requested byte range. Interior
// parts are taken whole; only the first and last are trimmed.
struct Part {
    // None: the in-memory first part of a rewrapped FLAC stream (the
    // native header, already built). Some: a CDN segment URL.
    url: Option<String>,
    skip: usize,
    take: usize,
}

// Which parts cover [start, end], and how much of each. `urls` and
// `sizes` are parallel, both starting with the init header — the
// concatenated file is exactly init followed by the segments, so a byte
// offset needs no special handling for it.
fn plan(urls: &[Option<String>], sizes: &[u64], start: u64, end: u64) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut offset = 0u64;
    for (url, &len) in urls.iter().zip(sizes) {
        let part_end = offset + len; // exclusive
        // Skip parts entirely before the range; stop once past it.
        if part_end > start && offset <= end {
            let from = start.saturating_sub(offset);
            let to = (end - offset + 1).min(len);
            parts.push(Part {
                url: url.clone(),
                skip: from as usize,
                take: (to - from) as usize,
            });
        }
        offset = part_end;
        if offset > end {
            break;
        }
    }
    parts
}

// Tracks currently being assembled, so concurrent requests for the same
// one wait on the first rather than each starting their own fetch.
type AssemblyGates = HashMap<(u64, Quality), Arc<tokio::sync::Mutex<()>>>;
static ASSEMBLING: LazyLock<Mutex<AssemblyGates>> =
    LazyLock::new(|| Mutex::new(AssemblyGates::new()));

// The tier to request for one track: the client's own hints if it sent
// any, else the configured default.
//
// This deliberately does *not* cap by the track's own metadata tier.
// An earlier version did, reasoning that asking for more than a track
// has wastes a round-trip — but Tidal downgrades server-side regardless
// (a LOSSLESS request on a lossy-only track simply comes back HIGH), so
// the cap cost a round-trip either way and only ever subtracted. It
// also actively broke hi-res: `mediaMetadata.tags` routinely carries
// just LOSSLESS on tracks Tidal will happily serve as HI_RES_LOSSLESS,
// so capping meant the ceiling could never be requested at all.
//
// Tidal is the authority on what it will serve; ask for the ceiling and
// report what comes back.
fn resolve_tier(q: &QueryParams) -> Quality {
    Quality::from_subsonic(q.max_bit_rate, q.format.as_deref()).unwrap_or_else(configured_tier)
}

// The `tidal_quality` setting, LOSSLESS when unset. An unrecognized
// value warns rather than silently serving a different tier.
fn configured_tier() -> Quality {
    let Some(raw) = crate::SETTINGS.get().map(|s| s.tidal_quality.as_str()) else {
        return Quality::Lossless;
    };
    Quality::from_setting(raw).unwrap_or_else(|| {
        tracing::warn!(
            "unrecognized tidal_quality {raw:?}; using LOSSLESS. \
             Valid values: LOW, HIGH, LOSSLESS, HI_RES_LOSSLESS, ATMOS"
        );
        Quality::Lossless
    })
}

pub async fn stream(
    q: QueryParams,
    range: Option<String>,
) -> Result<warp::reply::Response, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing").into_response());
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found").into_response());
    };
    let tier = resolve_tier(&q);
    let info = match resolve(track_id, tier).await {
        Ok(info) => info,
        Err(e) => return Ok(stream_error(track_id, e)),
    };
    Ok(serve(track_id, tier, info, range.as_deref(), None).await)
}

// download: the same resolution, asked for in offline mode like the
// official app's downloader. Subsonic allows several ids (a zip
// archive); this server builds no zip, so a multi-id request fails.
pub async fn download(
    q: QueryParams,
    range: Option<String>,
) -> Result<warp::reply::Response, warp::Rejection> {
    let ids = &q.id.0;
    if ids.is_empty() {
        return Ok(fail(10, "Required parameter missing").into_response());
    }
    if ids.len() > 1 {
        return Ok(fail(0, "Multiple downloads not supported").into_response());
    }
    let Some(track_id) = ids::parse_track_id(&ids[0]) else {
        return Ok(fail(70, "Song not found").into_response());
    };
    let tier = resolve_tier(&q);
    let client = crate::tidal::client();
    let info = match client.download_info(track_id, tier).await {
        Ok(info) => info,
        Err(e) => return Ok(stream_error(track_id, e)),
    };
    // Name the file after what it actually contains: the FLAC tiers are
    // rewrapped to a native .flac stream, the lossy ones stay MP4.
    let ext = if info.codec.starts_with("flac") {
        "flac"
    } else {
        "m4a"
    };
    let filename = format!("{track_id}.{ext}");
    Ok(serve(track_id, tier, info, range.as_deref(), Some(filename)).await)
}

// Resolve a track's manifest, reusing a recent one when there is one.
// A repeat within the TTL must not spend a manifest slot: a client
// re-requests the same track on pause/resume and after a failed load,
// and each of those would otherwise consume one of the limiter's
// 5-per-10s starts.
async fn resolve(track_id: u64, tier: Quality) -> Result<StreamInfo, Error> {
    if let Some(info) = cached_manifest(track_id, tier) {
        return Ok(info);
    }
    let info = crate::tidal::client()
        .stream_info(track_id, tier, "STREAM")
        .await?;
    store_manifest(track_id, tier, &info);
    Ok(info)
}

// Answer with the audio. The log line reports what Tidal actually
// served, not what was requested — the two differ whenever the account
// or the track lacks the tier, and that difference is the first thing
// worth knowing when a client plays the wrong quality.
async fn serve(
    track_id: u64,
    requested: Quality,
    info: StreamInfo,
    range: Option<&str>,
    attachment: Option<String>,
) -> warp::reply::Response {
    if info.encrypted {
        // The CDN bytes are AES-128-CTR ciphertext keyed by the
        // manifest's keyId. Serving them would hand the client noise it
        // would report as a corrupt file.
        tracing::error!(
            "track {track_id} came back encrypted (keyId set); \
             this client id is issued encrypted assets, which cannot be served"
        );
        return fail(0, "Stream unavailable").into_response();
    }
    // Print the wire spelling of the requested tier, not just the enum's
    // Debug form, so this line and the playbackinfo trace can be read
    // against each other without the two renderings looking like a
    // mismatch.
    tracing::debug!(
        "stream {track_id} requested={} served={:?} codec={} {:?} Hz {:?}-bit",
        requested.as_audioquality(),
        info.quality,
        info.codec,
        info.sample_rate,
        info.bit_depth
    );
    match info.asset {
        // Already a whole file: hand over the CDN URL and let the client
        // fetch and range-seek against Tidal directly. Nothing is gained
        // by copying those bytes through here.
        Asset::File(url) => redirect(url),
        // Segmented. Clients use /stream to download as well as to play,
        // and a playlist is not a file — one saved by a downloader is a
        // few KB of text pointing at URLs that expire within the hour.
        // Since the request carries no signal distinguishing the two,
        // the only shape that satisfies both is a real file.
        Asset::Segmented { init, segments } => {
            // Already assembled: serve from memory, which needs no
            // network at all and makes ranges trivial. The rewrapped
            // FLAC stream is FLAC bytes, so the type follows the codec.
            let content_type = if info.codec.starts_with("flac") {
                "audio/flac"
            } else {
                "audio/mp4"
            };
            if let Some(bytes) = cached_audio(track_id, requested) {
                return audio_reply(bytes, range, attachment, content_type);
            }
            if info.codec.starts_with("flac") {
                serve_flac(track_id, requested, init, segments, range, attachment).await
            } else {
                serve_mp4(track_id, requested, init, segments, range, attachment).await
            }
        }
    }
}

// Serve a segmented FLAC track as one native FLAC stream, when it must
// be built rather than served from cache.
async fn serve_flac(
    track_id: u64,
    tier: Quality,
    init: String,
    segments: Vec<String>,
    range: Option<&str>,
    attachment: Option<String>,
) -> warp::reply::Response {
    let init_bytes = match fetch_part(&init).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("track {track_id}: init segment failed: {e}");
            return fail(0, "Stream unavailable").into_response();
        }
    };
    let Some(header) = crate::flac::header(&init_bytes) else {
        // Not MP4-wrapped FLAC after all. Falling back beats serving a
        // file with a header we could not build.
        tracing::warn!(
            "track {track_id}: init segment is not MP4-wrapped FLAC; \
             buffering the track as fragmented MP4"
        );
        let count = segments.len();
        return match assemble(track_id, tier, init, segments).await {
            Ok(bytes) => {
                tracing::debug!("stream {track_id} assembled {count} segments");
                audio_reply(bytes, range, attachment, "audio/mp4")
            }
            Err(e) => {
                tracing::error!("track {track_id} could not be assembled: {e}");
                fail(0, "Stream unavailable").into_response()
            }
        };
    };

    // The rewrapped length, measured from box headers rather than by
    // downloading the audio. Without it the response has no
    // Content-Length, and a player that sent a Range refuses the reply
    // outright ("byte range and no content length").
    let Some(sizes) = flac_sizes(track_id, tier, &segments).await else {
        tracing::warn!(
            "track {track_id}: segment sizes unavailable; buffering the whole track"
        );
        return match assemble_flac(track_id, tier, header, segments).await {
            Ok(bytes) => audio_reply(bytes, range, attachment, "audio/flac"),
            Err(e) => {
                tracing::error!("track {track_id} could not be assembled: {e}");
                fail(0, "Stream unavailable").into_response()
            }
        };
    };

    // Part 0 is the header, held in memory; the rest are CDN segments.
    // The stream is exactly those concatenated, so a byte offset maps
    // onto them the same way it does for any file.
    let mut lens: Vec<u64> = Vec::with_capacity(sizes.len() + 1);
    lens.push(header.len() as u64);
    lens.extend(sizes.iter().copied());
    // Part 0 is the in-memory header (None); the rest are CDN segments.
    let parts_desc: Vec<Option<String>> =
        std::iter::once(None).chain(segments.iter().cloned().map(Some)).collect();
    chunked_reply(
        track_id,
        tier,
        parts_desc,
        lens,
        range,
        attachment,
        "audio/flac",
        Some(header),
        chunked_flac_transform,
    )
    .await
}

// Serve a segmented non-FLAC track, streaming the requested range.
async fn serve_mp4(
    track_id: u64,
    tier: Quality,
    init: String,
    segments: Vec<String>,
    range: Option<&str>,
    attachment: Option<String>,
) -> warp::reply::Response {
    let urls: Vec<String> = std::iter::once(init.clone())
        .chain(segments.iter().cloned())
        .collect();
    let Some(sizes) = sizes_for(track_id, tier, &urls).await else {
        tracing::debug!(
            "stream {track_id}: no segment size table; buffering the whole track"
        );
        let count = segments.len();
        return match assemble(track_id, tier, init, segments).await {
            Ok(bytes) => {
                tracing::debug!(
                    "stream {track_id} assembled {count} segments into {} bytes",
                    bytes.len()
                );
                audio_reply(bytes, range, attachment, "audio/mp4")
            }
            Err(e) => {
                tracing::error!("track {track_id} could not be assembled: {e}");
                fail(0, "Stream unavailable").into_response()
            }
        };
    };
    let parts_desc: Vec<Option<String>> = urls.into_iter().map(Some).collect();
    chunked_reply(
        track_id,
        tier,
        parts_desc,
        sizes.to_vec(),
        range,
        attachment,
        "audio/mp4",
        None,
        chunked_mp4_transform,
    )
    .await
}

// Fetch and concatenate a segmented track, reusing a cached body when
// one is warm. The cache is what keeps repeat requests cheap: a client
// re-requesting on every pause/resume would otherwise re-download the
// whole track from Tidal each time.
async fn assemble(
    track_id: u64,
    tier: Quality,
    init: String,
    segments: Vec<String>,
) -> Result<Arc<Vec<u8>>, Error> {
    if let Some(bytes) = cached_audio(track_id, tier) {
        return Ok(bytes);
    }
    // Single-flight. Clients routinely fire the same request two or
    // three times in a row (pause/resume, a retried load, a range probe
    // arriving before the first response). Without this each one starts
    // its own assembly, so N duplicate requests take N times the CDN
    // bandwidth and all of them finish slower than one would have.
    let gate = {
        let mut map = ASSEMBLING.lock().unwrap();
        Arc::clone(
            map.entry((track_id, tier))
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    };
    // The std lock is released above: holding it across the await below
    // would block every other track's cache lookups too.
    let _guard = gate.lock().await;

    // The winner of the gate filled the cache while this task waited.
    if let Some(bytes) = cached_audio(track_id, tier) {
        return Ok(bytes);
    }
    let result = crate::tidal::client().fetch_segments(init, segments).await;
    // Drop the gate entry whatever happened, so a failure does not
    // wedge the track behind a stale mutex until restart.
    ASSEMBLING.lock().unwrap().remove(&(track_id, tier));

    let bytes = Arc::new(result?);
    store_audio(track_id, tier, &bytes);
    Ok(bytes)
}

// Per-segment FLAC payload lengths, measured once and kept for the
// session. Sizes are a property of the asset, not of the signed URLs,
// so they outlive a manifest refetch.
async fn flac_sizes(track_id: u64, tier: Quality, segments: &[String]) -> Option<Arc<Vec<u64>>> {
    if let Some(sizes) = SIZE_CACHE.get(&(track_id, tier)) {
        return Some(sizes);
    }
    // A fragment's moof is a few hundred bytes; a kilobyte reaches the
    // mdat header behind it with room to spare.
    const PREFIX: u64 = 1024;
    let prefixes = crate::tidal::client()
        .fetch_prefixes(segments.to_vec(), PREFIX)
        .await;
    let sizes: Option<Vec<u64>> = prefixes
        .iter()
        .map(|p| p.as_ref().and_then(|b| crate::flac::mdat_len(b)))
        .collect();
    let sizes = sizes?;
    if sizes.len() != segments.len() || sizes.contains(&0) {
        return None;
    }
    let sizes = Arc::new(sizes);
    SIZE_CACHE.insert((track_id, tier), Arc::clone(&sizes));
    Some(sizes)
}

// Fetch every segment and build the whole FLAC stream. Only used when
// the sizes could not be measured cheaply, since it makes the client
// wait for the entire track.
async fn assemble_flac(
    track_id: u64,
    tier: Quality,
    header: Vec<u8>,
    segments: Vec<String>,
) -> Result<Arc<Vec<u8>>, Error> {
    use futures_util::StreamExt as _;
    // Fetched through the segment cache, in order, with the same
    // lookahead as the streaming path. The init segment is not included:
    // its contents are already in `header`, and passing an empty URL
    // here would send a request to nowhere.
    let parts: Vec<Result<bytes::Bytes, Error>> = futures_util::stream::iter(segments)
        .map(|url: String| async move { fetch_part(&url).await })
        .buffered(SEGMENT_CONCURRENCY)
        .collect()
        .await;
    let mut out = header;
    for part in parts {
        out.extend_from_slice(&crate::flac::frames(&part?));
    }
    let out = Arc::new(out);
    store_audio(track_id, tier, &out);
    Ok(out)
}

// The size table for a track, measured once and kept for the session.
async fn sizes_for(track_id: u64, tier: Quality, urls: &[String]) -> Option<Arc<Vec<u64>>> {
    if let Some(sizes) = SIZE_CACHE.get(&(track_id, tier)) {
        return Some(sizes);
    }
    let sizes = crate::tidal::client()
        .segment_sizes(urls.to_vec())
        .await?;
    // A part reporting zero length would desync every offset after it.
    if sizes.len() != urls.len() || sizes.contains(&0) {
        return None;
    }
    let sizes = Arc::new(sizes);
    SIZE_CACHE.insert((track_id, tier), Arc::clone(&sizes));
    Some(sizes)
}

// Turn one fetched part into the bytes contributed to the stream.
// Returns the transformed payload, or an error when the part is
// shorter than its advertised length — the CDN lied, and every later
// offset in the body would desync.
type SegmentTransform = fn(track_id: u64, raw: bytes::Bytes) -> Result<bytes::Bytes, Error>;

fn chunked_flac_transform(_track_id: u64, raw: bytes::Bytes) -> Result<bytes::Bytes, Error> {
    Ok(bytes::Bytes::from(crate::flac::frames(&raw)))
}

fn chunked_mp4_transform(_track_id: u64, raw: bytes::Bytes) -> Result<bytes::Bytes, Error> {
    Ok(raw)
}

// Stream the requested byte range as one continuous file, fetching only
// the parts that cover it and sending each on as it arrives.
//
// This is what keeps a 30 MB track from costing eight seconds of
// silence before playback starts: the client gets the first bytes after
// one part rather than after all ninety. Seeking still works because
// the size table gives an exact Content-Length, and a seek deep into a
// track now skips the parts before it instead of downloading them.
//
// `lens` are the part lengths in stream order; part 0 is the in-memory
// header for a rewrapped FLAC body (`header` carries its bytes), or the
// init segment URL for a raw MP4 body. `transform` rewraps each part:
// FLAC strips MP4 box framing, MP4 is the identity.
//
// The whole stream accumulates as it goes and is cached on completion,
// so the next request for the same track serves from memory with an
// exact length and full range support.
async fn chunked_reply(
    track_id: u64,
    tier: Quality,
    parts_desc: Vec<Option<String>>,
    lens: Vec<u64>,
    range: Option<&str>,
    attachment: Option<String>,
    content_type: &'static str,
    header: Option<Vec<u8>>,
    transform: SegmentTransform,
) -> warp::reply::Response {
    let total: u64 = lens.iter().sum();
    let (start, end, partial) = match range.and_then(|h| parse_range(h, total)) {
        Some((s, e)) => (s, e, true),
        None => (0, total.saturating_sub(1), false),
    };
    // `parts_desc` and `lens` are parallel; part 0 is the in-memory
    // header (None) for a rewrapped FLAC body.
    let parts = plan(&parts_desc, &lens, start, end);
    let length = end - start + 1;
    // How many of the needed parts are already warm. A run where this
    // stays at zero across repeated requests means the cache is not
    // doing its job, which is invisible from timings alone because the
    // headers go out before any body is fetched.
    let warm = parts
        .iter()
        .filter(|p| p.url.as_deref().is_some_and(|u| SEGMENT_CACHE.contains_key(u)))
        .count();
    tracing::debug!(
        "stream {track_id} streaming bytes {start}-{end}/{total} from {} of {} parts ({warm} cached)",
        parts.len(),
        lens.len()
    );

    // Whether this response covers the entire file, and so can populate
    // the whole-track cache. Note this is decided by the byte span, not
    // by whether a Range header was present: players ask for the whole
    // file *as* a range (`bytes=0-`), and treating that as partial meant
    // nothing was ever cached.
    let cacheable = start == 0 && end + 1 == total;
    // Headers go out before any body is fetched, so the response time in
    // the access log says nothing about how long the audio took. Time
    // the body itself: it is the only way to tell "the server is slow"
    // apart from "the client is still thinking".
    let began = Instant::now();
    let acc = Arc::new(Mutex::new(Vec::new()));
    let acc_body = Arc::clone(&acc);
    let acc_done = Arc::clone(&acc);
    let header_for_body = header.clone();

    // Fetched ahead but yielded in order, so the client receives a
    // continuous stream while later parts are still in flight. The
    // in-memory header is already in hand and needs no fetch.
    use futures_util::StreamExt as _;
    let body = futures_util::stream::iter(parts)
        .map(move |part: Part| {
            let header = header_for_body.clone();
            async move {
                let raw = match part.url {
                    None => {
                        // The header, already in memory.
                        let h = header.expect("header part without header bytes");
                        return Ok(bytes::Bytes::from(
                            h[part.skip..part.skip + part.take].to_vec(),
                        ));
                    }
                    Some(url) => fetch_part(&url).await?,
                };
                let payload = transform(track_id, raw)?;
                // A part shorter than its advertised length would shift
                // every later offset; stop rather than send audio that
                // does not line up with Content-Length.
                if part.skip + part.take > payload.len() {
                    return Err(Error::Malformed(format!(
                        "track {track_id}: segment shorter than its header claimed"
                    )));
                }
                Ok(payload.slice(part.skip..part.skip + part.take))
            }
        })
        .buffered(SEGMENT_CONCURRENCY)
        .map(move |chunk| match chunk {
            Ok(bytes) => {
                if cacheable {
                    acc_body.lock().unwrap().extend_from_slice(&bytes);
                }
                Ok(bytes)
            }
            Err(e) => {
                tracing::error!("stream {track_id} failed mid-body: {e}");
                // Ending the body early is the only signal available
                // once headers are sent; the client sees a short read.
                Err(std::io::Error::other(e.to_string()))
            }
        })
        // Runs only if the client read the whole body, which is what
        // makes it safe to treat the accumulator as complete.
        .chain(futures_util::stream::once(async move {
            if cacheable {
                let bytes = Arc::new(std::mem::take(&mut *acc_done.lock().unwrap()));
                tracing::debug!(
                    "stream {track_id} body complete: {} bytes in {:?}",
                    bytes.len(),
                    began.elapsed()
                );
                store_audio(track_id, tier, &bytes);
            }
            Ok(bytes::Bytes::new())
        }));

    let mut resp = warp::reply::stream(body).into_response();
    if partial {
        *resp.status_mut() = warp::http::StatusCode::PARTIAL_CONTENT;
    }
    let headers = resp.headers_mut();
    headers.insert("Content-Type", content_type.parse().unwrap());
    headers.insert("Content-Length", length.to_string().parse().unwrap());
    headers.insert("Accept-Ranges", "bytes".parse().unwrap());
    if partial {
        headers.insert(
            "Content-Range",
            format!("bytes {start}-{end}/{total}").parse().unwrap(),
        );
    }
    if let Some(name) = attachment {
        headers.insert(
            "Content-Disposition",
            format!("attachment; filename=\"{name}\"").parse().unwrap(),
        );
    }
    resp
}

// Serve assembled bytes, honouring a Range request so clients can seek
// without re-fetching the whole track. `Accept-Ranges` is always
// advertised: without it a player assumes the body is unseekable and
// disables scrubbing entirely. `content_type` follows what the body
// actually is: rewrapped FLAC serves as audio/flac, everything else as
// audio/mp4.
fn audio_reply(
    bytes: Arc<Vec<u8>>,
    range: Option<&str>,
    attachment: Option<String>,
    content_type: &'static str,
) -> warp::reply::Response {
    let total = bytes.len() as u64;
    let sliced = range.and_then(|h| parse_range(h, total));

    let (status, body, content_range) = match sliced {
        Some((start, end)) => (
            warp::http::StatusCode::PARTIAL_CONTENT,
            bytes[start as usize..=end as usize].to_vec(),
            Some(format!("bytes {start}-{end}/{total}")),
        ),
        // An unsatisfiable Range (past the end) is deliberately answered
        // with the whole body rather than a 416: players send odd ranges
        // while probing, and a hard error there stops playback outright.
        None => (warp::http::StatusCode::OK, bytes.as_ref().clone(), None),
    };

    let len = body.len();
    let mut resp = warp::reply::Response::new(body.into());
    *resp.status_mut() = status;
    let headers = resp.headers_mut();
    headers.insert("Content-Type", content_type.parse().unwrap());
    headers.insert("Content-Length", len.to_string().parse().unwrap());
    headers.insert("Accept-Ranges", "bytes".parse().unwrap());
    if let Some(cr) = content_range {
        headers.insert("Content-Range", cr.parse().unwrap());
    }
    if let Some(name) = attachment {
        headers.insert(
            "Content-Disposition",
            format!("attachment; filename=\"{name}\"").parse().unwrap(),
        );
    }
    resp
}

// One byte range from a Range header, clamped to the body. Returns the
// inclusive (start, end) pair, or None when the header is absent,
// malformed, multi-range, or starts past the end.
fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = header.trim().strip_prefix("bytes=")?;
    // Multi-range would need a multipart body; no audio client asks for
    // one, and answering the first range only would corrupt playback.
    if spec.contains(',') {
        return None;
    }
    let (from, to) = spec.split_once('-')?;
    let (start, end) = match (from.trim(), to.trim()) {
        // "bytes=-500": the *last* 500 bytes, not a range ending at 500.
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            (total.saturating_sub(n.min(total)), total - 1)
        }
        (s, "") => (s.parse().ok()?, total - 1),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(total - 1)),
    };
    if start > end || start >= total {
        return None;
    }
    Some((start, end))
}
fn stream_error(track_id: u64, e: Error) -> warp::reply::Response {
    if matches!(e, Error::RateLimited) {
        tracing::warn!("tidal stream limit hit for track {track_id}");
        return fail(0, "Stream unavailable").into_response();
    }
    if e.is_unavailable_asset() {
        tracing::warn!("track {track_id} not playable on tidal: {e}");
        return fail(70, "Song not found").into_response();
    }
    // The v1 endpoint distinguishes why an asset is refused; the
    // sub-status is the difference between "fix your subscription" and
    // "this is a bug", so name it rather than logging one opaque line.
    match sub_status(&e) {
        Some(4010) => tracing::warn!("track {track_id}: monthly stream quota exceeded"),
        Some(4032) | Some(4035) => {
            tracing::warn!("track {track_id}: not available in this region")
        }
        Some(4033) => {
            tracing::warn!("track {track_id}: not available on this subscription tier")
        }
        _ => tracing::error!("tidal stream fetch failed for track {track_id}: {e}"),
    }
    fail(0, "Stream unavailable").into_response()
}

// Tidal's `subStatus` from an error body, when it carried one.
fn sub_status(e: &Error) -> Option<u64> {
    let Error::Tidal(_, body) = e else {
        return None;
    };
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("subStatus")?
        .as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(max_bit_rate: Option<u32>, format: Option<&str>) -> QueryParams {
        QueryParams {
            max_bit_rate,
            format: format.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn the_configured_tier_is_requested_unchanged() {
        // The configured default is LOSSLESS in tests (SETTINGS unset).
        // Nothing about the track reduces it: an earlier version capped
        // by the track's mediaMetadata tier, which meant a hi-res
        // ceiling could never actually be requested, because those tags
        // routinely read LOSSLESS on tracks Tidal serves as hi-res.
        assert_eq!(resolve_tier(&params(None, None)), Quality::Lossless);
    }

    #[test]
    fn client_hints_win_over_the_configured_tier() {
        // VeloSonic's Atmos option sends eac3 with no bitrate cap.
        assert_eq!(resolve_tier(&params(None, Some("eac3"))), Quality::Atmos);
        // A bitrate cap is a real client constraint and is honored.
        assert_eq!(resolve_tier(&params(Some(128), None)), Quality::High);
        assert_eq!(resolve_tier(&params(Some(64), None)), Quality::Low);
        // The cap still wins over a format hint asking for more.
        assert_eq!(resolve_tier(&params(Some(128), Some("eac3"))), Quality::High);
    }

    #[test]
    fn an_unrelated_format_hint_keeps_the_configured_tier() {
        // A client sending format=mp3 is naming a codec it can play,
        // not a tier. Previously any non-empty format forced LOSSLESS.
        assert_eq!(resolve_tier(&params(None, Some("mp3"))), Quality::Lossless);
        // maxBitRate=0 means "no limit" in Subsonic, not a cap.
        assert_eq!(resolve_tier(&params(Some(0), None)), Quality::Lossless);
    }

    // All parts are CDN segments, so every URL is Some. A None first
    // part is the in-memory FLAC header, exercised separately below.
    fn urls(n: usize) -> Vec<Option<String>> {
        (0..n).map(|i| Some(format!("u{i}"))).collect()
    }

    #[test]
    fn a_full_range_plans_every_part_whole() {
        // init is 100 bytes, then three 200-byte segments = 700 total.
        let sizes = vec![100, 200, 200, 200];
        let p = plan(&urls(4), &sizes, 0, 699);
        assert_eq!(p.len(), 4);
        assert!(p.iter().all(|x| x.skip == 0));
        assert_eq!(p.iter().map(|x| x.take).sum::<usize>(), 700);
    }

    #[test]
    fn a_range_fetches_only_the_parts_it_covers() {
        let sizes = vec![100, 200, 200, 200];
        // Offsets: u0 0-99, u1 100-299, u2 300-499, u3 500-699.
        // Bytes 350-550 straddle u2 and u3. Fetching u0 and u1 too
        // would waste exactly what seeking is meant to save.
        let p = plan(&urls(4), &sizes, 350, 550);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].url.as_deref(), Some("u2"));
        assert_eq!(p[0].skip, 50);
        assert_eq!(p[0].take, 150);
        assert_eq!(p[1].url.as_deref(), Some("u3"));
        assert_eq!(p[1].skip, 0);
        assert_eq!(p[1].take, 51);
        // The slices must add up to the requested length exactly, or
        // Content-Length and the body disagree.
        assert_eq!(p.iter().map(|x| x.take).sum::<usize>(), 201);
    }

    #[test]
    fn a_range_inside_one_part_trims_both_ends() {
        let sizes = vec![100, 200, 200, 200];
        let p = plan(&urls(4), &sizes, 120, 179);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].url.as_deref(), Some("u1"));
        assert_eq!(p[0].skip, 20);
        assert_eq!(p[0].take, 60);
    }

    #[test]
    fn the_init_header_is_just_the_first_part() {
        // The concatenated file is init followed by segments, so byte 0
        // lands in init with no special casing.
        let sizes = vec![100, 200];
        let p = plan(&urls(2), &sizes, 0, 49);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].url.as_deref(), Some("u0"));
        assert_eq!(p[0].take, 50);
        // A range starting after init skips it entirely.
        let p = plan(&urls(2), &sizes, 100, 299);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].url.as_deref(), Some("u1"));
    }

    #[test]
    fn a_suffix_range_reaches_the_last_byte() {
        let sizes = vec![100, 200, 200, 200];
        let total: u64 = sizes.iter().sum();
        let (s, e) = parse_range("bytes=-100", total).unwrap();
        let p = plan(&urls(4), &sizes, s, e);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].url.as_deref(), Some("u3"));
        assert_eq!(p[0].skip, 100);
        assert_eq!(p[0].take, 100);
    }

    #[test]
    fn the_in_memory_flac_header_is_a_none_part() {
        // A rewrapped FLAC body is the in-memory header (None) followed
        // by the CDN segments. Byte ranges land on it like any other
        // part; the None marks it as already in hand.
        let parts_desc = vec![None, Some("seg0".into()), Some("seg1".into())];
        let sizes = vec![500, 200, 200];
        let p = plan(&parts_desc, &sizes, 0, 499);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].url, None);
        assert_eq!(p[0].skip, 0);
        assert_eq!(p[0].take, 500);
        // A range beyond the header skips it, hitting the segments.
        let p = plan(&parts_desc, &sizes, 700, 899);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].url.as_deref(), Some("seg1"));
        assert_eq!(p[0].skip, 0);
        assert_eq!(p[0].take, 200);
    }

    #[test]
    fn range_header_yields_an_inclusive_clamped_span() {
        // Both ends given.
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=100-199", 1000), Some((100, 199)));
        // Open end means "to the last byte".
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        // An end past the body is clamped, not rejected: players ask
        // for more than exists while probing.
        assert_eq!(parse_range("bytes=900-99999", 1000), Some((900, 999)));
        // A suffix range is the LAST n bytes, not a range ending at n.
        assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
        assert_eq!(parse_range("bytes=-99999", 1000), Some((0, 999)));
        // Whitespace around the spec is tolerated.
        assert_eq!(parse_range(" bytes=0-9 ", 1000), Some((0, 9)));
    }

    #[test]
    fn unusable_ranges_fall_back_to_the_whole_body() {
        // None means "serve it all", which is what a player probing with
        // a nonsense range should get instead of a hard 416.
        assert_eq!(parse_range("bytes=1000-1100", 1000), None, "starts past the end");
        assert_eq!(parse_range("bytes=500-100", 1000), None, "inverted");
        assert_eq!(parse_range("items=0-10", 1000), None, "wrong unit");
        assert_eq!(parse_range("bytes=abc-def", 1000), None);
        assert_eq!(parse_range("bytes=", 1000), None);
        assert_eq!(parse_range("nonsense", 1000), None);
        // Multi-range needs a multipart body; answering only the first
        // range would corrupt playback, so decline the whole thing.
        assert_eq!(parse_range("bytes=0-99,200-299", 1000), None);
        // An empty body has no satisfiable range.
        assert_eq!(parse_range("bytes=0-0", 0), None);
    }

    #[test]
    fn sub_status_is_read_from_the_error_body() {
        let e = Error::Tidal(401, r#"{"status":401,"subStatus":4033}"#.into());
        assert_eq!(sub_status(&e), Some(4033));
        assert_eq!(sub_status(&Error::Tidal(500, "<html>".into())), None);
        assert_eq!(sub_status(&Error::RateLimited), None);
    }
}
