// Album browsing: getAlbum, the album lists, and album info.
use super::favorites::favorites_albums;
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::{
    Album, AlbumId3, AlbumInfo, AlbumInfo2Response, AlbumInfoResponse, AlbumList,
    AlbumList2, AlbumList2Response, AlbumListResponse, AlbumWithSongs, Child, GetAlbumResponse,
};
use crate::navidrome::params::QueryParams;
use crate::navidrome::play_state;
use crate::tidal::client::FAVORITES_CAP;
use super::{fail, fail_with, ok};
use crate::tidal::mapping::{album_from_tidal, cover_url, song_from_track};

// getAlbum: one album plus its tracks in track order. The album's year
// uses v1 api
pub async fn get_album(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    // al<id>, or a bare number as a raw Tidal album id.
    let Some(album_id) = ids::decode(IdKind::Album, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Album not found"));
    };
    let client = crate::tidal::client();
    let (detail, songs) =
        match crate::tidal::client::TidalClient::album_detail_and_tracks(client, album_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("tidal album fetch failed: {e}");
                return Ok(fail_with(0, "Album unavailable", &e));
            }
        };
    let album = match album_from_tidal(&detail) {
        Some(a) => a,
        None => return Ok(fail(70, "Album not found")),
    };
    let year = album.year;
    let mut song: Vec<Child> = songs.iter().filter_map(song_from_track).collect();
    for s in &mut song {
        if s.year.is_none() {
            s.year = year;
        }
    }
    Ok(ok(GetAlbumResponse {
        album: AlbumWithSongs { album, song },
    }))
}

// The whole favorites list as AlbumID3 items, or the shared failure.
async fn all_favorite_albums() -> Result<Vec<AlbumId3>, String> {
    match crate::tidal::client::TidalClient::favorite_albums(crate::tidal::client(), 0, FAVORITES_CAP).await {
        Ok(v) => Ok(favorites_albums(&v)),
        Err(e) => {
            tracing::error!("tidal favorites fetch failed: {e}");
            Err(format!("Album list unavailable: {}", e.user_reason()))
        }
    }
}

fn page<T>(items: Vec<T>, offset: u32, size: u32) -> Vec<T> {
    items
        .into_iter()
        .skip(offset as usize)
        .take(size as usize)
        .collect()
}

// Albums from the local play history, in the record order given. Detail
// fetches run in parallel and hit the meta cache; an album Tidal no
// longer serves is dropped from the list.
async fn history_albums(records: Vec<play_state::PlayRecord>) -> Vec<AlbumId3> {
    let client = crate::tidal::client();
    let handles: Vec<_> = records
        .iter()
        .map(|r| {
            let album_id = r.album_id;
            tokio::spawn(async move { client.album(album_id).await })
        })
        .collect();
    let mut out = Vec::with_capacity(records.len());
    for (r, handle) in records.into_iter().zip(handles) {
        match handle.await {
            Ok(Ok(detail)) => out.extend(album_from_tidal(&detail)),
            Ok(Err(e)) => tracing::debug!("album {} dropped from history: {e}", r.album_id),
            Err(e) => tracing::debug!("album {} fetch task failed: {e}", r.album_id),
        }
    }
    out
}

// True when the album carries the genre, on either the single label or
// the OpenSubsonic genres list. Case-insensitive, like Navidrome.
fn has_genre(a: &AlbumId3, genre: &str) -> bool {
    a.genre.as_deref().is_some_and(|g| g.eq_ignore_ascii_case(genre))
        || a.genres
            .as_ref()
            .is_some_and(|gs| gs.iter().any(|g| g.name.eq_ignore_ascii_case(genre)))
}

// The list core shared by getAlbumList2 and getAlbumList. Returns the
// album list for the requested type, already paginated. The favorites
// list is the library; recent/frequent come from the local play history;
// newest is Tidal's personalised feed. highest stays empty: Tidal has no
// ratings.
async fn album_list_core(q: &QueryParams) -> Result<Vec<AlbumId3>, String> {
    let offset = q.offset.unwrap_or(0);
    let size = q.size.unwrap_or(10).min(500);
    let album: Vec<AlbumId3> = match q.r#type.as_deref() {
        Some("starred") => {
            match crate::tidal::client::TidalClient::favorite_albums(crate::tidal::client(), offset, size).await {
                Ok(v) => favorites_albums(&v),
                Err(e) => {
                    tracing::error!("tidal favorites fetch failed: {e}");
                    return Err(format!("Album list unavailable: {}", e.user_reason()));
                }
            }
        }
        Some("random") => {
            let mut album = all_favorite_albums().await?;
            crate::navidrome::handlers::jukebox::shuffle(&mut album);
            album.truncate(size as usize);
            album
        }
        Some("recent") => {
            history_albums(page(play_state::recent_albums(), offset, size)).await
        }
        Some("frequent") => {
            history_albums(page(play_state::frequent_albums(), offset, size)).await
        }
        Some("newest") => {
            let result = match crate::tidal::client().home_feed("static").await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal home feed fetch failed: {e}");
                    return Err(format!("Album list unavailable: {}", e.user_reason()));
                }
            };
            let raw = crate::tidal::client::albums_from_page(&result);
            raw.iter()
                .skip(offset as usize)
                .take(size as usize)
                .filter_map(album_from_tidal)
                .collect()
        }
        // The catalogue's albums for a Tidal browse genre; a label Tidal
        // has no browse page for falls back to the favorites carrying it.
        Some("byGenre") => {
            let Some(genre) = q.genre.as_deref() else {
                return Err("Required parameter missing: genre".into());
            };
            let client = crate::tidal::client();
            match crate::tidal::client::genre_key(client, genre).await {
                Some(key) => match crate::tidal::client::genre_albums(client, &key, offset, size).await {
                    Ok(items) => items.iter().filter_map(album_from_tidal).collect(),
                    Err(e) => {
                        tracing::error!("tidal genre albums fetch failed: {e}");
                        return Err(format!("Album list unavailable: {}", e.user_reason()));
                    }
                },
                None => {
                    let mut album = all_favorite_albums().await?;
                    album.retain(|a| has_genre(a, genre));
                    page(album, offset, size)
                }
            }
        }
        Some("byYear") => {
            let (Some(from), Some(to)) = (q.from_year, q.to_year) else {
                return Err("Required parameter missing: fromYear/toYear".into());
            };
            // A reversed window (fromYear > toYear) lists newest first.
            let (lo, hi) = (from.min(to), from.max(to));
            let mut album = all_favorite_albums().await?;
            album.retain(|a| a.year.is_some_and(|y| (lo..=hi).contains(&y)));
            album.sort_by_key(|a| a.year);
            if from > to {
                album.reverse();
            }
            page(album, offset, size)
        }
        Some("alphabeticalByName") => {
            let mut album = all_favorite_albums().await?;
            album.sort_by_key(|a| a.name.to_lowercase());
            page(album, offset, size)
        }
        Some("alphabeticalByArtist") => {
            let mut album = all_favorite_albums().await?;
            album.sort_by_key(|a| a.artist.to_lowercase());
            page(album, offset, size)
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
async fn album_info_core(q: &QueryParams) -> Result<AlbumInfo, (u32, String)> {
    let Some(id) = q.id.0.first() else {
        return Err((10, "Required parameter missing".into()));
    };
    let Some(album_id) = ids::decode(IdKind::Album, id).or_else(|| id.parse().ok()) else {
        return Err((70, "Album not found".into()));
    };
    let detail = match crate::tidal::client().album(album_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal album fetch failed: {e}");
            return Err((0, format!("Album unavailable: {}", e.user_reason())));
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
