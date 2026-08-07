// Subsonic endpoint handlers, one module per endpoint family. routes.rs
// stays flat and calls the re-exports below.
pub mod album;
pub mod annotate;
pub mod artist;
pub mod cover;
pub mod favorites;
pub mod jukebox;
pub mod playlist;
pub mod search;
pub mod system;
pub mod tracks;

use super::auth::Unauthorized;
use super::models::{SubsonicBody, SubsonicError, SubsonicErrorBody, SubsonicResponse};
use warp::reject::Rejection;

pub use album::{get_album, get_album_list2};
pub use annotate::scrobble;
pub use artist::{get_artist, get_artist_info2, get_top_songs};
pub use cover::get_cover_art;
pub use favorites::{get_starred, get_starred2, star, unstar};
pub use jukebox::jukebox_control;
pub use playlist::{get_genres, get_playlist, get_playlists};
pub use search::search3;
pub use system::{get_music_folders, get_open_subsonic_extensions, get_scan_status, get_user, ping};
pub use tracks::{get_random_songs, get_song, stream};

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
