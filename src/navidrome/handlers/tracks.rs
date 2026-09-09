// Track browsing: getSong, getRandomSongs, getSongsByGenre, getSimilarSongs
// (v1 and v2). Streaming lives in super::stream.
use super::favorites::favorite_track_songs;
use crate::navidrome::ids;
use crate::navidrome::models::{
    Child, GetSongResponse, RandomSongs, RandomSongsResponse, SimilarSongs, SimilarSongs2,
    SimilarSongs2Response, SimilarSongsResponse, SongsByGenre, SongsByGenreResponse,
};
use crate::navidrome::params::QueryParams;
use rand::seq::SliceRandom;
use std::collections::HashSet;
use super::{fail, ok};
use crate::tidal::client::Error;
use crate::tidal::mapping::{song_from_track, year_from};

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

#[cfg(test)]
mod tests {
    use super::pick_random;
    use crate::navidrome::models::{Child, GenreItem};

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
