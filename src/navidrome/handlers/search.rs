// search3: full-text search across artists, albums, and songs.
use crate::navidrome::models::{SearchResult3, SearchResult3Response};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::{album_from_tidal, artist_from_tidal, search_items, song_from_track};

pub async fn search3(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(query) = q.query else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if query.trim().is_empty() {
        return Ok(ok(SearchResult3Response {
            search_result: SearchResult3 {
                artist: vec![],
                album: vec![],
                song: vec![],
            },
        }));
    }
    let result = match crate::tidal::client().search(&query).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal search failed: {e}");
            return Ok(fail(0, "Search failed"));
        }
    };
    let slice = |section: &str, count: Option<u32>, offset: Option<u32>| {
        let count = count.unwrap_or(20) as usize;
        let offset = offset.unwrap_or(0) as usize;
        search_items(&result, section)
            .into_iter()
            .skip(offset)
            .take(count)
            .collect::<Vec<_>>()
    };
    Ok(ok(SearchResult3Response {
        search_result: SearchResult3 {
            artist: slice("artists", q.artist_count, q.artist_offset)
                .into_iter()
                .filter_map(artist_from_tidal)
                .collect(),
            album: slice("albums", q.album_count, q.album_offset)
                .into_iter()
                .filter_map(album_from_tidal)
                .collect(),
            song: slice("tracks", q.song_count, q.song_offset)
                .into_iter()
                .filter_map(song_from_track)
                .collect(),
        },
    }))
}
