// Subsonic endpoint handlers, one module per endpoint family. routes.rs
// stays flat and calls the re-exports below.
pub mod album;
pub mod annotate;
pub mod artist;
pub mod cover;
pub mod favorites;
pub mod jukebox;
pub mod lyrics;
pub mod now_playing;
pub mod playlist;
pub mod search;
pub mod system;
pub mod tracks;

use super::auth::Unauthorized;
use super::models::{SubsonicBody, SubsonicError, SubsonicErrorBody, SubsonicResponse};
use warp::reject::Rejection;
use warp::Reply;

pub use album::{
    get_album, get_album_info, get_album_info2, get_album_list, get_album_list2,
};
pub use annotate::{scrobble, set_rating};
pub use artist::{get_artist, get_artist_info, get_artist_info2, get_top_songs};
pub use cover::{get_avatar, get_cover_art};
pub use favorites::{get_starred, get_starred2, star, unstar};
pub use jukebox::jukebox_control;
pub use lyrics::{get_lyrics, get_lyrics_by_song_id};
pub use now_playing::{get_now_playing, report_playback, update_now_playing};
pub use playlist::{get_genres, get_playlist, get_playlists};
pub use search::{search2, search3};
pub use system::{
    get_license, get_music_folders, get_open_subsonic_extensions, get_scan_status, get_user,
    get_users, ping, start_scan,
};
pub use tracks::{
    get_random_songs, get_similar_songs, get_similar_songs2, get_song, get_songs_by_genre, stream,
};

// Response envelope helpers, shared by every handler.
pub(crate) fn ok<T: serde::Serialize>(data: T) -> warp::reply::Json {
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

// A 302 redirect reply. stream, getCoverArt, and getAvatar use it to
// point clients at Tidal's CDN without proxying bytes.
pub(crate) fn redirect(url: String) -> warp::reply::Response {
    warp::reply::with_header(
        warp::reply::with_status(warp::reply(), warp::http::StatusCode::FOUND),
        "Location",
        url,
    )
    .into_response()
}

pub(crate) fn fail(code: u32, message: &'static str) -> warp::reply::Json {
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

// Converts middleware rejections into Subsonic error replies.
// Any other rejection propagates to the 404 fallback route.
pub async fn recover(r: Rejection) -> Result<warp::reply::Json, Rejection> {
    if r.find::<Unauthorized>().is_some() {
        Ok(fail(40, "Wrong username or password"))
    } else {
        Err(r)
    }
}
