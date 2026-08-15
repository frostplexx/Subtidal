// System endpoints: ping, extension advertisement, and the user profile.
use crate::navidrome::models::{
    GetOpenSubsonicExtensionsResponse, GetUserResponse, GetUsersResponse, License,
    LicenseResponse, MusicFolder, MusicFolders, MusicFoldersResponse, OpenSubsonicExtension,
    PingResponse, ScanStatus, ScanStatusResponse, User, Users,
};
use crate::navidrome::params::QueryParams;
use super::ok;
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
            OpenSubsonicExtension { name: "transcodeDecision", versions: vec![1] },
            OpenSubsonicExtension { name: "playbackReport", versions: vec![1] },
            OpenSubsonicExtension { name: "topSongsByArtistId", versions: vec![1] },
        ],
    }))
}

// The Tidal account name and an email derived from it. Falls back to the
// configured username when the profile is unreachable.
async fn account_identity() -> (String, String) {
    let settings = SETTINGS.get().expect("settings not loaded");
    let fallback = settings.username.clone();
    match crate::tidal::client().user_profile().await {
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
    }
}

// The single user of this server. Role flags mirror what the bridge can
// actually do; scrobblingEnabled reflects the configured scrobble backends.
fn user_of(username: String, email: String, scrobbling_enabled: bool) -> User {
    User {
        folder: vec![1],
        username,
        email,
        scrobbling_enabled: if scrobbling_enabled { "true" } else { "false" },
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
    }
}

pub async fn get_user() -> Result<warp::reply::Json, warp::Rejection> {
    let (username, email) = account_identity().await;
    Ok(ok(GetUserResponse {
        user: user_of(username, email, crate::navidrome::scrobble::enabled()),
    }))
}

// getUsers: Navidrome returns only the user identified in the
// authentication; this server has exactly one user.
pub async fn get_users() -> Result<warp::reply::Json, warp::Rejection> {
    let (username, email) = account_identity().await;
    Ok(ok(GetUsersResponse {
        users: Users {
            user: vec![user_of(username, email, crate::navidrome::scrobble::enabled())],
        },
    }))
}

// getLicense: like Navidrome, the license is always valid.
pub async fn get_license() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(LicenseResponse {
        license: License { valid: true },
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

// startScan: nothing to scan, so the scan is instantly complete. The
// optional fullScan param is accepted and ignored.
pub async fn start_scan(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(ScanStatusResponse {
        scan_status: ScanStatus {
            scanning: false,
            count: 0,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_of_reflects_scrobbling_enabled() {
        let on = user_of("u".into(), "e".into(), true);
        assert_eq!(on.scrobbling_enabled, "true");
        let off = user_of("u".into(), "e".into(), false);
        assert_eq!(off.scrobbling_enabled, "false");
    }
}
