// Track browsing and streaming: getSong, getRandomSongs, getSongsByGenre,
// getSimilarSongs (v1 and v2), and stream.
use super::favorites::favorite_track_songs;
use crate::navidrome::ids;
use crate::navidrome::models::{
    Child, GetSongResponse, RandomSongs, RandomSongsResponse, SimilarSongs, SimilarSongs2,
    SimilarSongs2Response, SimilarSongsResponse, SongsByGenre, SongsByGenreResponse,
};
use crate::navidrome::params::QueryParams;
use crate::tidal::client::HlsInfo;
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

// The cache prevents a replay from re-fetching the manifest: Arpeggi
// re-requests the same URL after a failed track load, and a client
// bursting a queue would otherwise call playbackinfo once per song per
// retry. The CDN segment tokens inside carry short-lived signatures, so
// a stale entry degrades to a fresh fetch.
const HLS_TTL: Duration = Duration::from_secs(120);
type HlsCache = HashMap<(u64, String), (Instant, HlsInfo)>;
static HLS_CACHE: LazyLock<Mutex<HlsCache>> = LazyLock::new(|| Mutex::new(HlsCache::new()));

fn cached_hls(track_id: u64, tier: &str) -> Option<HlsInfo> {
    let mut map = HLS_CACHE.lock().unwrap();
    map.retain(|_, (at, _)| at.elapsed() < HLS_TTL);
    map.get(&(track_id, tier.to_string())).map(|(_, h)| h.clone())
}

fn store_hls(track_id: u64, tier: &str, hls: &HlsInfo) {
    let mut map = HLS_CACHE.lock().unwrap();
    map.retain(|_, (at, _)| at.elapsed() < HLS_TTL);
    // A replay of the same song re-signs the URLs; a single slot keeps
    // the cache small.
    map.insert((track_id, tier.to_string()), (Instant::now(), hls.clone()));
}

// stream: resolve a track to its v2 manifest and serve Tidal's native
// media playlist verbatim (v2 has no BTS single-file streams, so every
// request is segmented). The playlist points at Tidal's own CDN
// segments; no audio bytes cross this server. Ids are t<id> or bare
// numbers.
pub async fn stream(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
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
    // A replay of the same track within the TTL must not re-fetch the
    // manifest; serve the playlist the last fetch stored instead.
    if let Some(hls) = cached_hls(track_id, tier) {
        tracing::debug!(
            "stream {track_id} tier={tier} -> hls playlist (cached {} Hz, {}-bit {})",
            hls.sample_rate, hls.bit_depth, hls.codec
        );
        return Ok(hls_reply(hls.media_playlist));
    }
    match client.stream_info(track_id, tier, "STREAM").await {
        Ok(info) => {
            // A direct url is not expected on v2 (uriScheme=DATA), but
            // the parse defends against a manifest host change.
            if let Some(url) = info.direct_url {
                tracing::debug!("stream {track_id} tier={tier} -> redirect");
                return Ok(redirect(url));
            }
            if let Some(hls) = &info.hls {
                tracing::debug!(
                    "stream {track_id} tier={tier} -> hls playlist ({} Hz, {}-bit {})",
                    hls.sample_rate, hls.bit_depth, hls.codec
                );
                store_hls(track_id, tier, hls);
                return Ok(hls_reply(hls.media_playlist.clone()));
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
            if let Some(hls) = &info.hls {
                return Ok(hls_reply(hls.media_playlist.clone()));
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

#[cfg(test)]
mod tests {
    use super::{pick_random, tidal_quality};
    use crate::navidrome::models::{Child, GenreItem};

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
