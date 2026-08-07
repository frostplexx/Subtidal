// search2 / search3: full-text search across artists, albums, and songs.
// search3 returns the ID3 shapes, search2 the legacy Artist/Album shapes;
// both run the same Tidal search.
use serde_json::Value;

use crate::navidrome::models::{
    Album, Artist, SearchResult2, SearchResult2Response, SearchResult3, SearchResult3Response,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::{album_from_tidal, artist_from_tidal, search_items, song_from_track};

// Run the Tidal search and slice each section by its count/offset.
// Ok(None) is an empty query, which yields empty results without a
// search. Err carries the (code, message) for the failure.
async fn search_parts(
    q: &QueryParams,
) -> Result<Option<(Vec<Value>, Vec<Value>, Vec<Value>)>, (u32, &'static str)> {
    let Some(query) = q.query.as_deref() else {
        return Err((10, "Required parameter missing"));
    };
    if query.trim().is_empty() {
        return Ok(None);
    }
    let result = match crate::tidal::client().search(query).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal search failed: {e}");
            return Err((0, "Search failed"));
        }
    };
    let slice = |section: &str, count: Option<u32>, offset: Option<u32>| {
        let count = count.unwrap_or(20) as usize;
        let offset = offset.unwrap_or(0) as usize;
        search_items(&result, section)
            .into_iter()
            .skip(offset)
            .take(count)
            .cloned()
            .collect::<Vec<_>>()
    };
    Ok(Some((
        slice("artists", q.artist_count, q.artist_offset),
        slice("albums", q.album_count, q.album_offset),
        slice("tracks", q.song_count, q.song_offset),
    )))
}

pub async fn search3(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let empty = || SearchResult3 {
        artist: vec![],
        album: vec![],
        song: vec![],
    };
    let (artists, albums, songs) = match search_parts(&q).await {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(ok(SearchResult3Response { search_result: empty() })),
        Err((code, msg)) => return Ok(fail(code, msg)),
    };
    Ok(ok(SearchResult3Response {
        search_result: SearchResult3 {
            artist: artists.iter().filter_map(artist_from_tidal).collect(),
            album: albums.iter().filter_map(album_from_tidal).collect(),
            song: songs.iter().filter_map(song_from_track).collect(),
        },
    }))
}

pub async fn search2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let empty = || SearchResult2 {
        artist: vec![],
        album: vec![],
        song: vec![],
    };
    let (artists, albums, songs) = match search_parts(&q).await {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(ok(SearchResult2Response { search_result: empty() })),
        Err((code, msg)) => return Ok(fail(code, msg)),
    };
    Ok(ok(SearchResult2Response {
        search_result: SearchResult2 {
            artist: artists
                .iter()
                .filter_map(artist_from_tidal)
                .map(|a| Artist::from(&a))
                .collect(),
            album: albums
                .iter()
                .filter_map(album_from_tidal)
                .map(|a| Album::from(&a))
                .collect(),
            song: songs.iter().filter_map(song_from_track).collect(),
        },
    }))
}
