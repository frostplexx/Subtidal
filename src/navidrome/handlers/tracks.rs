// Track browsing and streaming: getSong, getRandomSongs, getSongsByGenre,
// getSimilarSongs (v1 and v2), and stream.
use super::favorites::favorite_track_songs;
use crate::navidrome::ids;
use crate::navidrome::models::{
    Child, GetSongResponse, RandomSongs, RandomSongsResponse, SimilarSongs, SimilarSongs2,
    SimilarSongs2Response, SimilarSongsResponse, SongsByGenre, SongsByGenreResponse,
};
use crate::navidrome::params::QueryParams;
use rand::seq::SliceRandom;
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
// offset/count. Tidal exposes no genre catalog (getGenres is empty), so
// this matches whatever genre string the track JSON carries.
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
// collection from the given artist and similar artists. The seed's top
// tracks plus the top tracks of the three closest similar artists are
// shuffled and truncated to count. A similar artist's fetch failure
// degrades to a warning; the seed's failure fails the request.
async fn similar_songs_core(q: &QueryParams) -> Result<Vec<Child>, (u32, &'static str)> {
    let Some(id) = q.id.0.first() else {
        return Err((10, "Required parameter missing"));
    };
    let Some(artist_id) = ids::decode(ids::IdKind::Artist, id).or_else(|| id.parse().ok()) else {
        return Err((70, "Artist not found"));
    };
    let count = q.count.unwrap_or(50).min(500) as usize;
    let client = crate::tidal::client();
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
            .map(|items| items.iter().filter_map(|a| a["id"].as_u64()).collect::<Vec<_>>())
            .unwrap_or_default(),
    );
    let per = (count / artists.len().max(1)).max(1) as u32;
    let mut songs: Vec<Child> = Vec::new();
    for (i, a) in artists.iter().enumerate() {
        match client.artist_top_tracks(*a, per).await {
            Ok(v) => songs.extend(
                v["items"]
                    .as_array()
                    .map(|items| items.iter().filter_map(song_from_track).collect::<Vec<_>>())
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
    songs.shuffle(&mut rand::rng());
    songs.truncate(count);
    Ok(songs)
}

pub async fn get_similar_songs2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match similar_songs_core(&q).await {
        Ok(song) => Ok(ok(SimilarSongs2Response {
            similar_songs2: SimilarSongs2 { song },
        })),
        Err((code, msg)) => Ok(fail(code, msg)),
    }
}

// getSimilarSongs v1: the same collection as v2 under the similarSongs
// wrapper.
pub async fn get_similar_songs(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match similar_songs_core(&q).await {
        Ok(song) => Ok(ok(SimilarSongsResponse {
            similar_songs: SimilarSongs { song },
        })),
        Err((code, msg)) => Ok(fail(code, msg)),
    }
}

// Map Subsonic maxBitRate (kbps) to a Tidal quality tier. Tidal has no
// real transcoding; the tier selects the stream the account can play.
// Bitrate wins when set: <=64 -> LOW (HE-AAC ~96k), <=320 -> HIGH (AAC
// 320k), anything above -> LOSSLESS. Without a bitrate, a lossy format
// hint caps at HIGH; flac or no format asks for LOSSLESS. Tidal cascades
// downward when a track or the account lacks the tier.
fn tidal_quality(max_bit_rate: Option<u32>, format: Option<&str>) -> &'static str {
    match max_bit_rate {
        // 0 means "no limit" in Subsonic; only a positive bitrate caps.
        Some(m) if (1..=64).contains(&m) => "LOW",
        Some(m) if (65..=320).contains(&m) => "HIGH",
        _ => match format {
            Some("flac") => "LOSSLESS",
            Some(f) if !f.is_empty() => "HIGH",
            _ => "LOSSLESS",
        },
    }
}

// The default tier when the client sends no bitrate and no format hint:
// the tidal_quality setting, LOSSLESS when unset or unknown.
fn default_tier() -> &'static str {
    match crate::SETTINGS.get().map(|s| s.tidal_quality.as_str()) {
        Some("HIGH") => "HIGH",
        Some("LOW") => "LOW",
        _ => "LOSSLESS",
    }
}

// stream: resolve a track to a Tidal CDN stream and serve it. Single-file
// BTS streams (AAC) 302-redirect; the server never touches the audio
// bytes. With format=hls the handler asks for HI_RES: hi-res tracks
// answer segmented DASH (FLAC 24-bit), which is rewritten into an HLS
// playlist pointing at Tidal's own init + segment URLs, so FLAC also
// flows client-to-CDN without server egress. Tidal has no AAC HLS, so
// hls requests ignore maxBitRate. Ids are t<id> or bare numbers.
pub async fn stream(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing").into_response());
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found").into_response());
    };
    let client = crate::tidal::client();
    let wants_hls = q.format.as_deref() == Some("hls");
    let tier = if wants_hls {
        "HI_RES"
    } else if q.max_bit_rate.is_none() && q.format.is_none() {
        default_tier()
    } else {
        tidal_quality(q.max_bit_rate, q.format.as_deref())
    };
    let mut tier: &str = tier;
    loop {
        match client.stream_info(track_id, tier, "STREAM").await {
            Ok(info) => {
                if let Some(url) = info.direct_url {
                    tracing::debug!("stream {track_id} tier={tier} -> redirect");
                    return Ok(redirect(url));
                }
                if wants_hls && let Some(dash) = &info.dash {
                    tracing::debug!(
                        "stream {track_id} tier={tier} -> hls playlist ({} Hz, {}-bit {})",
                        dash.sample_rate, dash.bit_depth, dash.codec
                    );
                    return Ok(hls_reply(build_hls_playlist(dash)));
                }
                tracing::debug!(
                    "tier {tier} returned {} for track {track_id}; retrying HIGH",
                    info.mime_type
                );
                if tier == "HIGH" {
                    break;
                }
                tier = "HIGH";
            }
            Err(e) => {
                if matches!(e, Error::RateLimited) {
                    tracing::warn!("tidal stream limit hit for track {track_id}");
                    break;
                }
                if e.is_unavailable_asset() {
                    tracing::warn!("track {track_id} not playable on tidal: {e}");
                    return Ok(fail(70, "Song not found").into_response());
                }
                tracing::error!("tidal stream fetch failed for track {track_id}: {e}");
                break;
            }
        }
    }
    Ok(fail(0, "Stream unavailable").into_response())
}

// download: a song as a 302 to its Tidal CDN stream URL, like stream.
// Subsonic allows several ids (a zip archive); the server builds no zip,
// so a multi-id request fails. Only direct single-file streams (BTS)
// serve as downloads; segmented hi-res DASH has no single URL and
// cascades a tier down to HIGH. The manifest is requested in offline
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
    let mut tier: &str = tier;
    loop {
        match client.download_info(track_id, tier).await {
            Ok(info) => {
                if let Some(url) = info.direct_url {
                    return Ok(redirect(url));
                }
                if tier == "HIGH" {
                    break;
                }
                tier = "HIGH";
            }
            Err(e) => {
                if matches!(e, Error::RateLimited) {
                    tracing::warn!("tidal stream limit hit for track {track_id}");
                    break;
                }
                if e.is_unavailable_asset() {
                    tracing::warn!("track {track_id} not playable on tidal: {e}");
                    return Ok(fail(70, "Song not found").into_response());
                }
                tracing::error!("tidal stream fetch failed for track {track_id}: {e}");
                break;
            }
        }
    }
    Ok(fail(0, "Stream unavailable").into_response())
}

fn hls_reply(playlist: String) -> warp::reply::Response {
    let reply = warp::reply::with_header(playlist, "Content-Type", "application/vnd.apple.mpegurl");
    warp::reply::with_header(reply, "Cache-Control", "no-store").into_response()
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
    use crate::navidrome::models::Child;
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
        assert_eq!(tidal_quality(None, Some("mp3")), "HIGH");
        assert_eq!(tidal_quality(Some(128), Some("flac")), "HIGH");
        assert_eq!(tidal_quality(Some(64), Some("mp3")), "LOW");
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
            cover_art: None,
            duration: 0,
            disc_number: None,
            album_id: String::new(),
            artist_id: String::new(),
            kind: "song",
            content_type: "audio/flac",
            suffix: "flac",
            size: 0,
            path: String::new(),
            created: String::new(),
            starred: None,
            starred_at: None,
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
