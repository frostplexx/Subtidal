// Track browsing and streaming: getSong, getRandomSongs, getSongsByGenre,
// getSimilarSongs (v1 and v2), and stream.
use super::favorites::favorite_track_songs;
use crate::navidrome::ids;
use crate::navidrome::models::{
    Child, GetSongResponse, RandomSongs, RandomSongsResponse, SimilarSongs, SimilarSongs2,
    SimilarSongs2Response, SimilarSongsResponse, SongsByGenre, SongsByGenreResponse,
};
use crate::navidrome::params::QueryParams;
use crate::tidal::client::DashInfo;
use rand::seq::SliceRandom;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use super::{fail, ok, redirect};
use crate::tidal::client::Error;
use crate::tidal::mapping::{song_from_track, year_from};
use warp::Reply;

// getSong: one track's detail. The id may be t<id> or a bare number.
// Tidal track JSON carries no release date (not even on the embedded
// album), so the year is filled from the album detail, mirroring getAlbum.
// The album fetch hits the meta cache, so repeat calls cost nothing.
pub async fn get_song(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found"));
    };
    let client = crate::tidal::client();
    let detail = match client.track(track_id).await {
        Ok(v) => v.to_json(),
        Err(e) => {
            tracing::error!("tidal track fetch failed: {e}");
            return Ok(fail(0, "Song unavailable"));
        }
    };
    let mut song = match song_from_track(&detail) {
        Some(s) => s,
        None => return Ok(fail(70, "Song not found")),
    };
    if song.year.is_none()
        && let Some(album_id) = detail["album"]["id"].as_u64()
        && let Ok(album) = client.album(album_id).await
    {
        song.year = year_from(album["releaseDate"].as_str());
    }
    Ok(ok(GetSongResponse { song }))
}

// getRandomSongs: shuffled favorites, the same random==favorites decision
// as getAlbumList2. genre and the fromYear/toYear window filter before
// the shuffle; musicFolderId is ignored (single virtual folder).
pub async fn get_random_songs(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let size = q.size.unwrap_or(10).min(500) as usize;
    let result = match crate::tidal::client().favorite_tracks(0, 2000).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal favorites fetch failed: {e}");
            return Ok(fail(0, "Favorites unavailable"));
        }
    };
    let songs = favorite_track_songs(&result);
    let song = pick_random(songs, size, q.genre.as_deref(), q.from_year, q.to_year);
    Ok(ok(RandomSongsResponse {
        random_songs: RandomSongs { song },
    }))
}

// Filter, shuffle, then truncate. Genre matches exactly; songs without a
// year pass the year window.
fn pick_random(
    mut songs: Vec<Child>,
    size: usize,
    genre: Option<&str>,
    from_year: Option<u32>,
    to_year: Option<u32>,
) -> Vec<Child> {
    songs.retain(|s| {
        genre.is_none_or(|g| s.genre.as_deref() == Some(g))
            && from_year.is_none_or(|f| s.year.is_none_or(|y| y >= f))
            && to_year.is_none_or(|t| s.year.is_none_or(|y| y <= t))
    });
    songs.shuffle(&mut rand::rng());
    songs.truncate(size);
    songs
}

// getSongsByGenre: favorite tracks filtered by genre, paginated by
// offset/count. The genre string is the label the track JSON carries.
pub async fn get_songs_by_genre(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(genre) = q.genre.as_deref() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let count = q.count.unwrap_or(10).min(500) as usize;
    let offset = q.offset.unwrap_or(0) as usize;
    let result = match crate::tidal::client().favorite_tracks(0, 2000).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal favorites fetch failed: {e}");
            return Ok(fail(0, "Favorites unavailable"));
        }
    };
    let song: Vec<Child> = favorite_track_songs(&result)
        .into_iter()
        .filter(|s| s.genre.as_deref() == Some(genre))
        .skip(offset)
        .take(count)
        .collect();
    Ok(ok(SongsByGenreResponse {
        songs_by_genre: SongsByGenre { song },
    }))
}

// The core shared by getSimilarSongs and getSimilarSongs2: a random
// collection of songs similar to the artist. The primary source is
// Tidal's own similar feed: the similarTracks relationship, seeded from
// the artist's most popular track. A short or empty feed pads with the
// old heuristic (top tracks of the seed and its three closest similar
// artists), deduped against the feed. A similar artist's fetch failure
// degrades to a warning; the seed's failure fails the request.
fn similar_songs_core(q: QueryParams) -> super::BoxedTryFuture<Vec<Child>, (u32, &'static str)> {
    Box::pin(async move {
    let Some(id) = q.id.0.first() else {
        return Err((10, "Required parameter missing"));
    };
    let Some(artist_id) = ids::decode(ids::IdKind::Artist, id).or_else(|| id.parse().ok()) else {
        return Err((70, "Artist not found"));
    };
    let count = q.count.unwrap_or(50).min(500) as usize;
    let client = crate::tidal::client();

    // The real feed first. On failure the heuristic below still runs, so
    // a broken relationship endpoint degrades instead of failing.
    let mut songs: Vec<Child> = match similar_feed_songs(client, artist_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("tidal similar feed failed for artist {artist_id}: {e}");
            Vec::new()
        }
    };

    if songs.len() < count {
        let known: HashSet<u64> = songs
            .iter()
            .filter_map(|s| ids::parse_track_id(&s.id))
            .collect();
        let similar = match client.artist_similar(artist_id, 3).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("tidal similar artists fetch failed: {e}");
                return Err((0, "Similar songs unavailable"));
            }
        };
        let mut artists: Vec<u64> = vec![artist_id];
        artists.extend(
            similar["items"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|a| a["id"].as_u64())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        );
        // Per-artist slice targets the remaining slots, capped upward so
        // a short feed still requests one track per artist.
        let per = ((count - songs.len()) / artists.len().max(1)).max(1) as u32;
        for (i, a) in artists.iter().enumerate() {
            match crate::tidal::client::TidalClient::artist_top_tracks_parallel(client, *a, per).await {
                Ok(v) => songs.extend(
                    v["items"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(song_from_track)
                                .filter(|s| {
                                    ids::parse_track_id(&s.id).is_none_or(|t| !known.contains(&t))
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                ),
                Err(e) => {
                    if i == 0 {
                        tracing::error!("tidal top tracks fetch failed: {e}");
                        return Err((0, "Similar songs unavailable"));
                    }
                    tracing::warn!("tidal top tracks failed for artist {a}: {e}");
                }
            }
        }
    }
    songs.shuffle(&mut rand::rng());
    songs.truncate(count);
    Ok(songs)
    })
}

// Tidal's similarTracks relationship for the artist's most popular
// track. Songs derive from the flattened feed. An empty feed (no top
// track or no mappable items) returns an empty list so the caller can
// pad from the heuristic.
async fn similar_feed_songs(
    client: &'static crate::tidal::client::TidalClient,
    artist_id: u64,
) -> Result<Vec<Child>, Error> {
    let top = crate::tidal::client::TidalClient::artist_top_tracks_parallel(client, artist_id, 1)
        .await?;
    let Some(seed_id) = top["items"][0]["id"].as_u64() else {
        return Ok(Vec::new());
    };
    let feed = client.track_similar(seed_id, 200).await?;
    Ok(feed["items"]
        .as_array()
        .map(|items| items.iter().filter_map(song_from_track).collect())
        .unwrap_or_default())
}

pub async fn get_similar_songs2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match similar_songs_core(q).await {
        Ok(song) => Ok(ok(SimilarSongs2Response {
            similar_songs2: SimilarSongs2 { song },
        })),
        Err((code, msg)) => Ok(fail(code, msg)),
    }
}

// getSimilarSongs v1: the same collection as v2 under the similarSongs
// wrapper.
pub async fn get_similar_songs(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match similar_songs_core(q).await {
        Ok(song) => Ok(ok(SimilarSongsResponse {
            similar_songs: SimilarSongs { song },
        })),
        Err((code, msg)) => Ok(fail(code, msg)),
    }
}

//TODO: turn tidal qualities into an enum.

// Map Subsonic maxBitRate (kbps) to a Tidal quality tier. A client
// requesting the "Dolby Atmos" transcode format (VeloSonic's
// StreamFormat.EAC3) sends format=eac3 with no maxBitRate (its bitrate is
// always ORIGINAL/unlimited for that format) — that must resolve to the
// ATMOS tier specifically, not fall through to the generic non-empty-
// format-means-LOSSLESS case below, or Tidal is never actually asked for
// an Atmos-formatted manifest at all.
fn tidal_quality(max_bit_rate: Option<u32>, format: Option<&str>) -> &'static str {
    match max_bit_rate {
        // 0 means "no limit" in Subsonic; only a positive bitrate caps.
        Some(m) if (1..=64).contains(&m) => "LOW",
        Some(m) if (65..=320).contains(&m) => "HIGH",
        _ => match format {
            Some("eac3") => "ATMOS",
            Some("flac") => "LOSSLESS",
            Some(f) if !f.is_empty() => "LOSSLESS",
            _ => "LOSSLESS",
        },
    }
}

// The default tier when the client sends no bitrate and no format hint:
// the tidal_quality setting, LOSSLESS when unset or unknown.
fn default_tier() -> &'static str {
    match crate::SETTINGS.get().map(|s| s.tidal_quality.as_str()) {
        Some("ATMOS") => "ATMOS",
        Some("HIGH") => "HIGH",
        Some("LOW") => "LOW",
        _ => "LOSSLESS",
    }
}

// The variant re-request must not re-fetch the manifest: a song would
// cost two playbackinfo calls, and a client bursting a queue would
// throttle at double speed. The master stores the parsed manifest here;
// the variant request, milliseconds later, hits the cache. CDN URLs
// carry short-lived signatures, so a stale entry degrades to a fresh
// fetch.
const DASH_TTL: Duration = Duration::from_secs(120);
type DashCache = HashMap<(u64, String), (Instant, DashInfo)>;
static DASH_CACHE: LazyLock<Mutex<DashCache>> = LazyLock::new(|| Mutex::new(DashCache::new()));

fn cached_dash(track_id: u64, tier: &str) -> Option<DashInfo> {
    let mut map = DASH_CACHE.lock().unwrap();
    map.retain(|_, (at, _)| at.elapsed() < DASH_TTL);
    map.get(&(track_id, tier.to_string())).map(|(_, d)| d.clone())
}

fn store_dash(track_id: u64, tier: &str, dash: &DashInfo) {
    let mut map = DASH_CACHE.lock().unwrap();
    map.retain(|_, (at, _)| at.elapsed() < DASH_TTL);
    // A replay of the same song re-signs the URLs; a single slot keeps
    // the cache small.
    map.insert((track_id, tier.to_string()), (Instant::now(), dash.clone()));
}

// stream: resolve a track to its v2 manifest and serve it as an HLS
// multivariant playlist on the first visit; the variant re-request (the
// same URL plus variant=1) gets the media playlist of Tidal's own CDN
// segment URLs (v2 has no BTS single-file streams, so every request is
// segmented). The playlist points at the init + numbered segments; no
// audio bytes cross this server. Ids are t<id> or bare numbers. The
// variant URL is absolute (scheme from x-forwarded-proto, host from the
// Host header) per the HLS multivariant spec; it falls back to a
// relative path when the host header is absent.
pub async fn stream(
    q: QueryParams,
    raw: String,
    proto: Option<String>,
    host: Option<String>,
) -> Result<warp::reply::Response, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing").into_response());
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found").into_response());
    };
    let client = crate::tidal::client();
    let tier = if q.max_bit_rate.is_none() && q.format.is_none() {
        default_tier()
    } else {
        tidal_quality(q.max_bit_rate, q.format.as_deref())
    };
    // A variant re-request carries the exact query of the master; serve
    // the manifest the master just stored instead of asking Tidal again.
    if q.variant.is_some()
        && let Some(dash) = cached_dash(track_id, tier)
    {
        tracing::debug!(
            "stream {track_id} tier={tier} -> hls playlist (cached {} Hz, {}-bit {})",
            dash.sample_rate, dash.bit_depth, dash.codec
        );
        return Ok(hls_reply(build_hls_playlist(&dash)));
    }
    match client.stream_info(track_id, tier, "STREAM").await {
        Ok(info) => {
            // A direct url is not expected on v2 (uriScheme=DATA), but
            // the parse defends against a manifest host change.
            if let Some(url) = info.direct_url {
                tracing::debug!("stream {track_id} tier={tier} -> redirect");
                return Ok(redirect(url));
            }
            if let Some(dash) = &info.dash {
                tracing::debug!(
                    "stream {track_id} tier={tier} -> hls playlist ({} Hz, {}-bit {})",
                    dash.sample_rate, dash.bit_depth, dash.codec
                );
                store_dash(track_id, tier, dash);
                return if q.variant.is_some() {
                    Ok(hls_reply(build_hls_playlist(dash)))
                } else {
                    let variant = variant_url(
                        &raw,
                        &q,
                        proto.as_deref(),
                        host.as_deref(),
                    );
                    Ok(hls_reply(build_hls_master(dash, &variant)))
                };
            }
            tracing::debug!("manifest for track {track_id} carried no playable stream");
        }
        Err(e) => {
            if matches!(e, Error::RateLimited) {
                tracing::warn!("tidal stream limit hit for track {track_id}");
                return Ok(fail(0, "Stream unavailable").into_response());
            }
            if e.is_unavailable_asset() {
                tracing::warn!("track {track_id} not playable on tidal: {e}");
                return Ok(fail(70, "Song not found").into_response());
            }
            tracing::error!("tidal stream fetch failed for track {track_id}: {e}");
        }
    }
    Ok(fail(0, "Stream unavailable").into_response())
}

// download: a song's manifest served as an HLS playlist, like stream.
// Subsonic allows several ids (a zip archive); the server builds no zip,
// so a multi-id request fails. The manifest is requested in offline
// mode, like the official app's downloader; a mode rejection falls back
// to the streaming mode.
pub async fn download(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
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
    let client = crate::tidal::client();
    let tier = if q.max_bit_rate.is_none() && q.format.is_none() {
        default_tier()
    } else {
        tidal_quality(q.max_bit_rate, q.format.as_deref())
    };
    match client.download_info(track_id, tier).await {
        Ok(info) => {
            if let Some(url) = info.direct_url {
                return Ok(redirect(url));
            }
            if let Some(dash) = &info.dash {
                return Ok(hls_reply(build_hls_playlist(dash)));
            }
        }
        Err(e) => {
            if matches!(e, Error::RateLimited) {
                tracing::warn!("tidal stream limit hit for track {track_id}");
                return Ok(fail(0, "Stream unavailable").into_response());
            }
            if e.is_unavailable_asset() {
                tracing::warn!("track {track_id} not playable on tidal: {e}");
                return Ok(fail(70, "Song not found").into_response());
            }
            tracing::error!("tidal stream fetch failed for track {track_id}: {e}");
        }
    }
    Ok(fail(0, "Stream unavailable").into_response())
}

fn hls_reply(playlist: String) -> warp::reply::Response {
    let reply = warp::reply::with_header(playlist, "Content-Type", "application/vnd.apple.mpegurl");
    warp::reply::with_header(reply, "Cache-Control", "no-store").into_response()
}

// Absolute URL for the media-playlist re-request. The raw query string
// carries the client's exact auth and tier params; variant=1 marks the
// re-request so the handler serves the media playlist instead of another
// master. When the raw query is empty (a formPost request), rebuild the
// auth/tier subset from the typed params so the variant still resolves.
fn variant_url(raw: &str, q: &QueryParams, proto: Option<&str>, host: Option<&str>) -> String {
    let query = if raw.is_empty() {
        let mut pairs: Vec<(String, String)> = Vec::new();
        if let Some(u) = &q.u {
            pairs.push(("u".into(), u.clone()));
        }
        if let Some(t) = &q.t {
            pairs.push(("t".into(), t.clone()));
        }
        if let Some(s) = &q.s {
            pairs.push(("s".into(), s.clone()));
        }
        if let Some(v) = &q.v {
            pairs.push(("v".into(), v.clone()));
        }
        if let Some(id) = q.id.0.first() {
            pairs.push(("id".into(), id.clone()));
        }
        if let Some(f) = &q.format {
            pairs.push(("format".into(), f.clone()));
        }
        if let Some(m) = &q.max_bit_rate {
            pairs.push(("maxBitRate".into(), m.to_string()));
        }
        pairs.push(("variant".into(), "1".into()));
        serde_urlencoded::to_string(&pairs).unwrap_or_default()
    } else {
        format!("{raw}&variant=1")
    };
    match host {
        Some(h) => {
            let proto = proto
                .and_then(|p| p.split(',').next())
                .map(str::trim)
                .filter(|p| *p == "http" || *p == "https")
                .unwrap_or("http");
            format!("{proto}://{h}/rest/stream?{query}")
        }
        None => format!("/rest/stream?{query}"),
    }
}

// RFC 6381 codec identifier for one DASH codec id. The ISO-BMFF
// fourcc for FLAC is "fLaC"; a lowercase "flac" fails AVFoundation's
// HLS loader with Cannot Open (-11848), which kills the whole
// multivariant playlist. Unknown codecs drop CODECS entirely: the
// attribute is optional and its absence plays cleanly.
fn hls_codecs(codec: &str) -> Option<&str> {
    match codec {
        "flac" => Some("fLaC"),
        // E-AC-3 / Dolby Atmos JOC: the MPD writes the RFC 6381 form
        // directly ("ec-3"), Apple's own HLS examples use it too.
        "ec-3" => Some("ec-3"),
        "eac3" => Some("ec-3"),
        "ac-3" => Some("ac-3"),
        "ac3" => Some("ac-3"),
        "mp4a" => Some("mp4a.40.2"),
        _ if codec.starts_with("mp4a.") => Some(codec),
        _ => None,
    }
}

// A one-variant HLS multivariant playlist: the required BANDWIDTH plus
// the recommended AVERAGE-BANDWIDTH and CODECS (RFC 6381, from the
// DASH representation), then the variant playlist as a full URL.
fn build_hls_master(dash: &crate::tidal::client::DashInfo, variant: &str) -> String {
    let stream_inf = match hls_codecs(&dash.codec) {
        Some(codecs) => format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},AVERAGE-BANDWIDTH={},CODECS=\"{}\"",
            dash.bandwidth, dash.bandwidth, codecs
        ),
        None => format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},AVERAGE-BANDWIDTH={}",
            dash.bandwidth, dash.bandwidth
        ),
    };
    format!("#EXTM3U\n#EXT-X-VERSION:6\n{stream_inf}\n{variant}\n")
}

// Rewrite a parsed DASH manifest into a VOD HLS media playlist. The init
// segment and each numbered segment keep Tidal's absolute CDN URLs.
fn build_hls_playlist(dash: &crate::tidal::client::DashInfo) -> String {
    let mut out = String::new();
    out.push_str("#EXTM3U\n");
    out.push_str("#EXT-X-VERSION:6\n");
    out.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    let target = dash
        .segments
        .iter()
        .map(|s| (s.samples as f64 / dash.timescale as f64).ceil() as u64)
        .max()
        .unwrap_or(4);
    out.push_str(&format!("#EXT-X-TARGETDURATION:{target}\n"));
    out.push_str(&format!("#EXT-X-MAP:URI=\"{}\"\n", dash.init_url));
    let mut number = dash.start_number;
    for seg in &dash.segments {
        let dur = seg.samples as f64 / dash.timescale as f64;
        for _ in 0..seg.count {
            out.push_str(&format!("#EXTINF:{dur:.3},\n"));
            out.push_str(&format!(
                "{}\n",
                dash.media_url.replace("$Number$", &number.to_string())
            ));
            number += 1;
        }
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}

#[cfg(test)]
mod tests {
    use super::{pick_random, tidal_quality};
    use crate::navidrome::models::{Child, GenreItem};
    use crate::tidal::client::{DashInfo, Segment};

    // Fixture matching the live hi-res MPD: 52 segments of 176128
    // samples at 44100 plus one partial of 118808.
    fn dash_fixture() -> DashInfo {
        DashInfo {
            bandwidth: 1616237,
            init_url: "https://cdn/init/0.mp4?token=t".into(),
            media_url: "https://cdn/media/$Number$.mp4?token=t".into(),
            timescale: 44100,
            start_number: 1,
            segments: vec![
                Segment {
                    samples: 176128,
                    count: 52,
                },
                Segment {
                    samples: 118808,
                    count: 1,
                },
            ],
            codec: "flac".into(),
            sample_rate: 44100,
            bit_depth: 24,
        }
    }

    #[test]
    fn hls_playlist_shape() {
        let pl = super::build_hls_playlist(&dash_fixture());
        let lines: Vec<&str> = pl.lines().collect();
        assert_eq!(lines[0], "#EXTM3U");
        assert!(pl.contains("#EXT-X-VERSION:6\n"));
        assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:VOD\n"));
        assert!(pl.contains("#EXT-X-TARGETDURATION:4\n"));
        assert!(pl.contains("#EXT-X-MAP:URI=\"https://cdn/init/0.mp4?token=t\"\n"));
        assert!(pl.contains("#EXT-X-ENDLIST\n"));
        // 52 full segments at 3.994s + 1 partial at 2.694s.
        assert_eq!(pl.matches("#EXTINF:3.994,").count(), 52);
        assert_eq!(pl.matches("#EXTINF:2.694,").count(), 1);
        assert!(pl.contains("https://cdn/media/1.mp4?token=t"));
        assert!(pl.contains("https://cdn/media/53.mp4?token=t"));
        assert!(!pl.contains("$Number$"));
    }

    #[test]
    fn variant_url_builds_absolute_with_host_and_proto() {
        let q = crate::navidrome::params::QueryParams {
            u: Some("admin".into()),
            t: Some("abc123".into()),
            s: Some("salt".into()),
            id: crate::navidrome::params::IdList(vec!["t123".into()]),
            ..Default::default()
        };
        let url = super::variant_url(
            "u=admin&t=abc123&s=salt&id=t123&v=1.16.1&c=Arpeggi",
            &q,
            Some("https"),
            Some("music.example.com:8443"),
        );
        assert_eq!(
            url,
            "https://music.example.com:8443/rest/stream?u=admin&t=abc123&s=salt&id=t123&v=1.16.1&c=Arpeggi&variant=1"
        );
    }

    #[test]
    fn variant_url_falls_back_to_relative_and_rebuilds_empties() {
        let q = crate::navidrome::params::QueryParams {
            u: Some("admin".into()),
            t: Some("abc123".into()),
            s: Some("salt".into()),
            v: Some("1.16.1".into()),
            id: crate::navidrome::params::IdList(vec!["t123".into()]),
            format: Some("flac".into()),
            max_bit_rate: Some(0),
            ..Default::default()
        };
        let url = super::variant_url("", &q, Some("https"), None);
        assert_eq!(
            url,
            "/rest/stream?u=admin&t=abc123&s=salt&v=1.16.1&id=t123&format=flac&maxBitRate=0&variant=1"
        );
        let url = super::variant_url("", &q, Some("https"), Some("localhost:8000"));
        assert_eq!(
            url,
            "https://localhost:8000/rest/stream?u=admin&t=abc123&s=salt&v=1.16.1&id=t123&format=flac&maxBitRate=0&variant=1"
        );
    }

    #[test]
    fn hls_master_has_bandwidth_and_codec_and_variant_url() {
        let master = super::build_hls_master(&dash_fixture(), "https://h/rest/stream?a=1&variant=1");
        let lines: Vec<&str> = master.lines().collect();
        assert_eq!(lines[0], "#EXTM3U");
        assert!(master.contains("#EXT-X-VERSION:6\n"));
        // Flac must use the RFC 6381 form: lowercase "flac" fails
        // AVFoundation's loader with Cannot Open (-11848).
        assert!(
            master.contains(
                "#EXT-X-STREAM-INF:BANDWIDTH=1616237,AVERAGE-BANDWIDTH=1616237,CODECS=\"fLaC\"\n"
            )
        );
        assert!(!master.contains("CODECS=\"flac\""));
        assert_eq!(lines[3], "https://h/rest/stream?a=1&variant=1");
        // No media tags in the multivariant playlist.
        assert!(!master.contains("EXTINF"));
        assert!(!master.contains("EXT-X-MAP"));
    }

    #[test]
    fn hls_codecs_maps_dash_ids_to_rfc6381() {
        assert_eq!(super::hls_codecs("flac"), Some("fLaC"));
        assert_eq!(super::hls_codecs("ec-3"), Some("ec-3"));
        assert_eq!(super::hls_codecs("eac3"), Some("ec-3"));
        assert_eq!(super::hls_codecs("ac-3"), Some("ac-3"));
        assert_eq!(super::hls_codecs("ac3"), Some("ac-3"));
        assert_eq!(super::hls_codecs("mp4a"), Some("mp4a.40.2"));
        assert_eq!(super::hls_codecs("mp4a.40.2"), Some("mp4a.40.2"));
        // Unknown codecs drop the CODECS attribute rather than risk
        // another loader rejection.
        assert_eq!(super::hls_codecs("weird"), None);
    }

    #[test]
    fn hls_master_omits_codecs_for_unknown_identifiers() {
        let mut dash = dash_fixture();
        dash.codec = "weird".into();
        let master = super::build_hls_master(&dash, "https://h/rest/stream?variant=1");
        assert!(master.contains("#EXT-X-STREAM-INF:BANDWIDTH=1616237,AVERAGE-BANDWIDTH=1616237\n"));
        assert!(!master.contains("CODECS"));
    }

    #[test]
    fn bitrate_picks_tier() {
        assert_eq!(tidal_quality(None, None), "LOSSLESS");
        assert_eq!(tidal_quality(Some(0), None), "LOSSLESS");
        assert_eq!(tidal_quality(Some(64), None), "LOW");
        assert_eq!(tidal_quality(Some(128), None), "HIGH");
        assert_eq!(tidal_quality(Some(320), None), "HIGH");
        assert_eq!(tidal_quality(Some(999), None), "LOSSLESS");
    }

    #[test]
    fn format_hint_only_matters_without_bitrate() {
        assert_eq!(tidal_quality(None, Some("flac")), "LOSSLESS");
        assert_eq!(tidal_quality(None, Some("mp3")), "LOSSLESS");
        assert_eq!(tidal_quality(Some(128), Some("flac")), "HIGH");
        assert_eq!(tidal_quality(Some(64), Some("mp3")), "LOW");
    }

    #[test]
    fn eac3_format_hint_resolves_to_atmos_tier() {
        // format=eac3 with no maxBitRate (VeloSonic's "Dolby Atmos" transcode
        // option always sends ORIGINAL/unlimited bitrate for this format) must
        // reach ATMOS specifically, not the generic non-empty-format-means-
        // LOSSLESS fallback every other unrecognized format hits.
        assert_eq!(tidal_quality(None, Some("eac3")), "ATMOS");
        // A bitrate cap still wins over the eac3 hint, same as any other format.
        assert_eq!(tidal_quality(Some(128), Some("eac3")), "HIGH");
    }

    // A minimal Child for pick_random tests.
    fn song(id: &str, year: Option<u32>, genre: Option<&str>) -> Child {
        Child {
            id: id.into(),
            parent: String::new(),
            is_dir: false,
            is_video: false,
            title: String::new(),
            album: String::new(),
            artist: String::new(),
            track: 0,
            year,
            genre: genre.map(String::from),
            genres: genre.map(|g| vec![GenreItem { name: g.to_string() }]),
            cover_art: None,
            duration: 0,
            bit_rate: None,
            bit_depth: None,
            sampling_rate: None,
            channel_count: None,
            disc_number: None,
            album_id: String::new(),
            artist_id: String::new(),
            artists: None,
            isrc: None,
            kind: "song",
            content_type: "audio/flac",
            suffix: "flac",
            size: 0,
            path: String::new(),
            created: String::new(),
            starred: None,
            starred_at: None,
            explicit_status: None,
            replay_gain: crate::navidrome::models::ReplayGain::default(),
        }
    }

    #[test]
    fn random_songs_truncates_to_size() {
        let songs = vec![song("a", None, None), song("b", None, None), song("c", None, None)];
        let picked = pick_random(songs, 2, None, None, None);
        assert_eq!(picked.len(), 2);
    }

    #[test]
    fn random_songs_filters_by_genre_and_year() {
        let songs = vec![
            song("a", Some(2005), Some("Rock")),
            song("b", Some(2010), Some("Jazz")),
            song("c", Some(2015), Some("Rock")),
        ];
        let picked = pick_random(songs, 10, Some("Rock"), Some(2010), Some(2020));
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "c");
    }

    #[test]
    fn random_songs_keeps_all_ids_when_shuffling() {
        let songs = vec![song("a", None, None), song("b", None, None), song("c", None, None)];
        let picked = pick_random(songs, 10, None, None, None);
        let mut ids: Vec<&str> = picked.iter().map(|s| s.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }
}
