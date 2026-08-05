use super::auth;
use super::models::{
    GetUserResponse, PingResponse, SubsonicBody, SubsonicError, SubsonicErrorBody,
    SubsonicResponse, User,
};
use super::params::QueryParams;

fn ok<T: serde::Serialize>(data: T) -> warp::reply::Json {
    warp::reply::json(&SubsonicResponse {
        inner: SubsonicBody {
            status: "ok",
            version: "1.16.1",
            server_type: "HighTide",
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
            server_type: "HighTide",
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
    let Some(username) = q.username else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if username != auth::USERNAME {
        return Ok(fail(70, "The requested data was not found"));
    }
    Ok(ok(GetUserResponse {
        user: User {
            folder: vec![1],
            username: "admin",
            email: "admin@localhost",
            scrobbling_enabled: "true",
            admin_role: "true",
            settings_role: "true",
            download_role: "true",
            upload_role: "true",
            playlist_role: "true",
            cover_art_role: "true",
            comment_role: "true",
            podcast_role: "true",
            stream_role: "true",
            jukebox_role: "true",
            share_role: "true",
        },
    }))
}
