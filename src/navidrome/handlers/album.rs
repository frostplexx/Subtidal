// Album browsing: getAlbum, the album lists, and album info.
use super::favorites::favorites_albums;
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::{
    Album, AlbumId3, AlbumInfo, AlbumInfo2Response, AlbumInfoResponse, AlbumList,
    AlbumList2, AlbumList2Response, AlbumListResponse, AlbumWithSongs, Child, GetAlbumResponse,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::{album_from_tidal, cover_url, song_from_track};

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

// The list core shared by getAlbumList2 and getAlbumList. Returns the
// album list for the requested type, already paginated.
async fn album_list_core(q: &QueryParams) -> Result<Vec<AlbumId3>, &'static str> {
    let offset = q.offset.unwrap_or(0);
    let size = q.size.unwrap_or(10).min(500);
    let album: Vec<AlbumId3> = match q.r#type.as_deref() {
        // starred/frequent/recent/byGenre page favorites directly; random
        // shuffles the same page.
        Some("starred" | "frequent" | "recent" | "byGenre" | "random") => {
            let result = match crate::tidal::client().favorite_albums(offset, size).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal favorites fetch failed: {e}");
                    return Err("Album list unavailable");
                }
            };
            let mut album = favorites_albums(&result);
            if q.r#type.as_deref() == Some("random") {
                crate::navidrome::handlers::jukebox::shuffle(&mut album);
            }
            album
        }
        Some("newest") => {
            let result = match crate::tidal::client().home_feed("static").await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal home feed fetch failed: {e}");
                    return Err("Album list unavailable");
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
                    return Err("Album list unavailable");
                }
            };
            let mut album = favorites_albums(&result);
            match q.r#type.as_deref() {
                Some("alphabeticalByName") => {
                    album.sort_by_key(|a| a.name.to_lowercase())
                }
                Some("alphabeticalByArtist") => {
                    album.sort_by_key(|a| a.artist.to_lowercase())
                }
                _ => {
                    let year = q.from_year.unwrap_or(0);
                    album.retain(|a| a.year == Some(year));
                }
            }
            album.into_iter().skip(offset as usize).take(size as usize).collect()
        }
        _ => Vec::new(),
    };
    Ok(album)
}

pub async fn get_album_list2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match album_list_core(&q).await {
        Ok(album) => Ok(ok(AlbumList2Response {
            album_list: AlbumList2 { album },
        })),
        Err(msg) => Ok(fail(0, msg)),
    }
}

// getAlbumList v1: the same list types as v2, served with the legacy
// Album shape.
pub async fn get_album_list(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match album_list_core(&q).await {
        Ok(album) => Ok(ok(AlbumListResponse {
            album_list: AlbumList {
                album: album.iter().map(Album::from).collect(),
            },
        })),
        Err(msg) => Ok(fail(0, msg)),
    }
}

// The info core shared by getAlbumInfo and getAlbumInfo2: album artwork
// at the three documented sizes. Tidal exposes no album notes and no
// external ids, so those stay empty and are omitted.
async fn album_info_core(q: &QueryParams) -> Result<AlbumInfo, (u32, &'static str)> {
    let Some(id) = q.id.0.first() else {
        return Err((10, "Required parameter missing"));
    };
    let Some(album_id) = ids::decode(IdKind::Album, id).or_else(|| id.parse().ok()) else {
        return Err((70, "Album not found"));
    };
    let detail = match crate::tidal::client().album(album_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal album fetch failed: {e}");
            return Err((0, "Album unavailable"));
        }
    };
    let cover = detail["cover"].as_str();
    Ok(AlbumInfo {
        notes: String::new(),
        music_brainz_id: String::new(),
        last_fm_url: String::new(),
        small_image_url: cover.map(|c| cover_url(c, 160)),
        medium_image_url: cover.map(|c| cover_url(c, 320)),
        large_image_url: cover.map(|c| cover_url(c, 640)),
    })
}

pub async fn get_album_info(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match album_info_core(&q).await {
        Ok(album_info) => Ok(ok(AlbumInfoResponse { album_info })),
        Err((code, msg)) => Ok(fail(code, msg)),
    }
}

pub async fn get_album_info2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match album_info_core(&q).await {
        Ok(album_info) => Ok(ok(AlbumInfo2Response { album_info })),
        Err((code, msg)) => Ok(fail(code, msg)),
    }
}
