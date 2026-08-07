// System endpoints: ping, extension advertisement, and the user profile.
use crate::navidrome::models::{
    GetOpenSubsonicExtensionsResponse, GetUserResponse, MusicFolder, MusicFolders,
    MusicFoldersResponse, OpenSubsonicExtension, PingResponse, ScanStatus, ScanStatusResponse, User,
};
use super::{ok};
use crate::SETTINGS;

pub async fn ping() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(PingResponse {}))
}

// getOpenSubsonicExtensions: advertise the OpenSubsonic extensions the server
// supports. Mirrors Navidrome v0.63.2 (server/subsonic/opensubsonic.go).
// Public endpoint: Navidrome serves it without authentication.
pub async fn get_open_subsonic_extensions() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(GetOpenSubsonicExtensionsResponse {
        extensions: vec![
            OpenSubsonicExtension { name: "transcodeOffset", versions: vec![1] },
            OpenSubsonicExtension { name: "formPost", versions: vec![1] },
            OpenSubsonicExtension { name: "songLyrics", versions: vec![1, 2] },
            OpenSubsonicExtension { name: "indexBasedQueue", versions: vec![1] },
            OpenSubsonicExtension { name: "transcoding", versions: vec![1] },
            OpenSubsonicExtension { name: "playbackReport", versions: vec![1] },
            OpenSubsonicExtension { name: "topSongsByArtistId", versions: vec![1] },
        ],
    }))
}

pub async fn get_user() -> Result<warp::reply::Json, warp::Rejection> {
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

// getMusicFolders: one virtual folder covering the whole Tidal catalog.
pub async fn get_music_folders() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(MusicFoldersResponse {
        music_folders: MusicFolders {
            music_folder: vec![MusicFolder { id: 1, name: "Tidal" }],
        },
    }))
}

// getScanStatus: there is no local library, so nothing scans. Arpeggi
// may prompt for a scan after seeing count 0.
pub async fn get_scan_status() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(ScanStatusResponse {
        scan_status: ScanStatus {
            scanning: false,
            count: 0,
        },
    }))
}
