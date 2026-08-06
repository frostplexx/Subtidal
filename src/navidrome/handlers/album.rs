// Album browsing: getAlbum and the album list.
use super::favorites::favorites_albums;
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::{
    AlbumId3, AlbumList2, AlbumList2Response, AlbumWithSongs, Child, GetAlbumResponse,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::{album_from_tidal, song_from_track};

// getAlbum: one album plus its tracks in track order. The album's year
// fills in for tracks, which carry no release date of their own.
pub async fn get_album(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    // al<id>, or a bare number as a raw Tidal album id.
    let Some(album_id) = ids::decode(IdKind::Album, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Album not found"));
    };
    let client = crate::tidal::client();
    let detail = match client.album(album_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal album fetch failed: {e}");
            return Ok(fail(0, "Album unavailable"));
        }
    };
    let tracks = match client.album_tracks(album_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal album tracks fetch failed: {e}");
            return Ok(fail(0, "Album unavailable"));
        }
    };
    let album = match album_from_tidal(&detail) {
        Some(a) => a,
        None => return Ok(fail(70, "Album not found")),
    };
    let year = album.year;
    let mut song: Vec<Child> = tracks["items"]
        .as_array()
        .map(|items| items.iter().filter_map(song_from_track).collect())
        .unwrap_or_default();
    for s in &mut song {
        if s.year.is_none() {
            s.year = year;
        }
    }
    Ok(ok(GetAlbumResponse {
        album: AlbumWithSongs { album, song },
    }))
}

// getAlbumList2: Subsonic's album listing. The mapping mirrors the
// working TidalDrome reference: starred/frequent/recent/byGenre map to
// favorites, newest maps to the personalized home feed (which includes the
// "Suggested new albums for you" section), random shuffles favorites, and
// alphabeticalByName/alphabeticalByArtist sort favorites. byYear filters
// favorites by fromYear. All favorites are fetched for the sorted and
// filtered types, then paginated locally.
pub async fn get_album_list2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let offset = q.offset.unwrap_or(0);
    let size = q.size.unwrap_or(10).min(500);
    let album: Vec<AlbumId3> = match q.r#type.as_deref() {
        Some("starred" | "frequent" | "recent" | "byGenre") => {
            let result = match crate::tidal::client().favorite_albums(offset, size).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal favorites fetch failed: {e}");
                    return Ok(fail(0, "Album list unavailable"));
                }
            };
            favorites_albums(&result)
        }
        Some("random") => {
            let result = match crate::tidal::client().favorite_albums(offset, size).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal favorites fetch failed: {e}");
                    return Ok(fail(0, "Album list unavailable"));
                }
            };
            let mut album = favorites_albums(&result);
            crate::navidrome::handlers::jukebox::shuffle(&mut album);
            album
        }
        Some("newest") => {
            let result = match crate::tidal::client().home_feed("static").await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal home feed fetch failed: {e}");
                    return Ok(fail(0, "Album list unavailable"));
                }
            };
            let raw = crate::tidal::client::albums_from_page(&result);
            raw.iter()
                .skip(offset as usize)
                .take(size as usize)
                .filter_map(album_from_tidal)
                .collect()
        }
        Some("alphabeticalByName" | "alphabeticalByArtist" | "byYear") => {
            let result = match crate::tidal::client().favorite_albums(0, 2000).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal favorites fetch failed: {e}");
                    return Ok(fail(0, "Album list unavailable"));
                }
            };
            let mut album = favorites_albums(&result);
            match q.r#type.as_deref() {
                Some("alphabeticalByName") => album.sort_by(|a, b| {
                    a.name.to_lowercase().cmp(&b.name.to_lowercase())
                }),
                Some("alphabeticalByArtist") => album.sort_by(|a, b| {
                    a.artist.to_lowercase().cmp(&b.artist.to_lowercase())
                }),
                _ => {
                    let year = q.from_year.unwrap_or(0);
                    album.retain(|a| a.year == Some(year));
                }
            }
            album.into_iter().skip(offset as usize).take(size as usize).collect()
        }
        _ => Vec::new(),
    };
    Ok(ok(AlbumList2Response {
        album_list: AlbumList2 { album },
    }))
}
