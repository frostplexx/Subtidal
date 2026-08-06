use super::auth;
use super::ids::{self, IdKind};
use super::models::{
    GetUserResponse, PingResponse, SearchResult3, SearchResult3Response, SubsonicBody,
    SubsonicError, SubsonicErrorBody, SubsonicResponse, User,
};
use super::params::QueryParams;
use crate::SETTINGS;
use crate::tidal::mapping::{album_from_tidal, artist_from_tidal, artist_pic_url, cover_url, search_items, song_from_track};
use warp::Reply;

fn ok<T: serde::Serialize>(data: T) -> warp::reply::Json {
    warp::reply::json(&SubsonicResponse {
        inner: SubsonicBody {
            status: "ok",
            version: "1.16.1",
            server_type: "Subtidal",
            server_version: "0.1.0",
            open_subsonic: true,
            data,
        },
    })
}

fn fail(code: u32, message: &'static str) -> warp::reply::Json {
    warp::reply::json(&SubsonicResponse {
        inner: SubsonicErrorBody {
            status: "failed",
            version: "1.16.1",
            server_type: "Subtidal",
            server_version: "0.1.0",
            open_subsonic: true,
            error: SubsonicError { code, message },
        },
    })
}

pub async fn ping() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(PingResponse {}))
}

pub async fn get_user(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    if !auth::authenticate(&q) {
        return Ok(fail(40, "Wrong username or password"));
    }
    // The response describes the Tidal account, whatever username the
    // client passed. Fall back to the configured username when the
    // profile is unreachable.
    let settings = SETTINGS.get().expect("settings not loaded");
    let fallback = settings.username.clone();
    let (username, email) = match crate::tidal::client().user_profile().await {
        Ok(v) => {
            let name = v["username"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| v["profileName"].as_str().filter(|s| !s.is_empty()))
                .unwrap_or(&fallback)
                .to_string();
            // Tidal's username is often the login email itself.
            let email = if name.contains('@') {
                name.clone()
            } else {
                format!("{name}@localhost")
            };
            (name, email)
        }
        Err(e) => {
            tracing::warn!("user profile fetch failed: {e}");
            (fallback.clone(), format!("{fallback}@localhost"))
        }
    };
    Ok(ok(GetUserResponse {
        user: User {
            folder: vec![1],
            username,
            email,
            scrobbling_enabled: "false", // flips on when scrobble middleware is configured
            admin_role: "true",
            settings_role: "true",
            download_role: "true",
            playlist_role: "true",
            cover_art_role: "true",
            stream_role: "true",
            upload_role: "false",
            comment_role: "false",
            podcast_role: "false",
            jukebox_role: "false",
            share_role: "false",
        },
    }))
}

// getCoverArt: resolve al<id>/ar<id>/bare album id to a Tidal image URL and
// 302-redirect there. The server never proxies image bytes.
pub async fn get_cover_art(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
    if !auth::authenticate(&q) {
        return Ok(fail(40, "Wrong username or password").into_response());
    }
    let Some(id) = q.id else {
        return Ok(fail(10, "Required parameter missing").into_response());
    };
    let size = q.size.unwrap_or(640);
    let (kind, raw_id) = match ids::parse(&id) {
        Some(kv) => kv,
        // Bare number = raw Tidal album ID (Subsonic convention).
        None => match id.parse::<u64>() {
            Ok(n) => (IdKind::Album, n),
            Err(_) => return Ok(fail(70, "Cover art not found").into_response()),
        },
    };
    let uuid = match kind {
        IdKind::Album => match crate::tidal::client().album(raw_id).await {
            Ok(v) => v["cover"].as_str().map(String::from),
            Err(e) => {
                tracing::warn!("album fetch failed: {e}");
                return Ok(fail(0, "Cover art unavailable").into_response());
            }
        },
        IdKind::Artist => match crate::tidal::client().artist(raw_id).await {
            Ok(v) => v["picture"].as_str().map(String::from),
            Err(e) => {
                tracing::warn!("artist fetch failed: {e}");
                return Ok(fail(0, "Cover art unavailable").into_response());
            }
        },
        // Tracks carry no own cover; playlists come with the playlists work.
        _ => None,
    };
    let Some(uuid) = uuid else {
        return Ok(fail(70, "Cover art not found").into_response());
    };
    let url = match kind {
        IdKind::Artist => artist_pic_url(&uuid, size),
        _ => cover_url(&uuid, size),
    };
    Ok(warp::reply::with_status(
        warp::reply::with_header(warp::reply(), "Location", url),
        warp::http::StatusCode::FOUND,
    )
    .into_response())
}

pub async fn search3(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    if !auth::authenticate(&q) {
        return Ok(fail(40, "Wrong username or password"));
    }
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
