// Response envelope, system endpoints, genres, and jukebox models.
use serde::Serialize;

use super::song::Child;

// Root of every response: { "subsonic-response": { ... } }
#[derive(Serialize)]
pub struct SubsonicResponse<T: Serialize> {
    #[serde(rename = "subsonic-response")]
    pub inner: T,
}

// Common fields inside subsonic-response, plus per-endpoint data
#[derive(Serialize)]
pub struct SubsonicBody<T: Serialize> {
    pub status: &'static str,
    pub version: &'static str, // supported Subsonic API version
    #[serde(rename = "type")] // "type" is a Rust keyword
    pub server_type: &'static str,
    #[serde(rename = "serverVersion")]
    pub server_version: &'static str,
    #[serde(rename = "openSubsonic")]
    pub open_subsonic: bool,
    #[serde(flatten)]
    pub data: T,
}

// ping returns no extra data
#[derive(Serialize)]
pub struct PingResponse {}

// getOpenSubsonicExtensions data: { openSubsonicExtensions: [ { name, versions } ] }
#[derive(Serialize)]
pub struct GetOpenSubsonicExtensionsResponse {
    #[serde(rename = "openSubsonicExtensions")]
    pub extensions: Vec<OpenSubsonicExtension>,
}

#[derive(Serialize)]
pub struct OpenSubsonicExtension {
    pub name: &'static str,
    pub versions: Vec<u32>,
}

// Error body: { status: "failed", ..., error: { code, message } }
#[derive(Serialize)]
pub struct SubsonicErrorBody {
    pub status: &'static str,
    pub version: &'static str,
    #[serde(rename = "type")]
    pub server_type: &'static str,
    #[serde(rename = "serverVersion")]
    pub server_version: &'static str,
    #[serde(rename = "openSubsonic")]
    pub open_subsonic: bool,
    pub error: SubsonicError,
}

#[derive(Serialize)]
pub struct SubsonicError {
    pub code: u32,
    pub message: &'static str,
}

// getUser data: { user: { ... } }
#[derive(Serialize)]
pub struct GetUserResponse {
    pub user: User,
}

// Role flags are strings ("true"/"false") to match the documented
// OpenSubsonic JSON output, a legacy Subsonic quirk.
#[derive(Serialize)]
pub struct User {
    pub folder: Vec<u32>,
    pub username: String,
    pub email: String,
    #[serde(rename = "scrobblingEnabled")]
    pub scrobbling_enabled: &'static str,
    #[serde(rename = "adminRole")]
    pub admin_role: &'static str,
    #[serde(rename = "settingsRole")]
    pub settings_role: &'static str,
    #[serde(rename = "downloadRole")]
    pub download_role: &'static str,
    #[serde(rename = "uploadRole")]
    pub upload_role: &'static str,
    #[serde(rename = "playlistRole")]
    pub playlist_role: &'static str,
    #[serde(rename = "coverArtRole")]
    pub cover_art_role: &'static str,
    #[serde(rename = "commentRole")]
    pub comment_role: &'static str,
    #[serde(rename = "podcastRole")]
    pub podcast_role: &'static str,
    #[serde(rename = "streamRole")]
    pub stream_role: &'static str,
    #[serde(rename = "jukeboxRole")]
    pub jukebox_role: &'static str,
    #[serde(rename = "shareRole")]
    pub share_role: &'static str,
}

// getGenres data: { genres: { genre: [ Genre ] } }
#[derive(Serialize)]
pub struct GenresResponse {
    pub genres: Genres,
}

#[derive(Serialize)]
pub struct Genres {
    pub genre: Vec<Genre>,
}

#[derive(Serialize)]
pub struct Genre {
    pub name: String,
    #[serde(rename = "songCount", skip_serializing_if = "Option::is_none")]
    pub song_count: Option<u32>,
    #[serde(rename = "albumCount", skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u32>,
}

// jukeboxControl data: jukeboxStatus (all actions); get also returns
// jukeboxPlaylist.
#[derive(Serialize)]
pub struct JukeboxControlResponse {
    #[serde(rename = "jukeboxStatus")]
    pub status: JukeboxStatus,
    #[serde(rename = "jukeboxPlaylist", skip_serializing_if = "Option::is_none")]
    pub playlist: Option<JukeboxPlaylist>,
}

#[derive(Serialize)]
pub struct JukeboxStatus {
    #[serde(rename = "currentIndex")]
    pub current_index: u32,
    pub playing: bool,
    pub gain: f32,
    pub position: u32,
}

#[derive(Serialize)]
pub struct JukeboxPlaylist {
    pub entry: Vec<Child>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_response_has_subsonic_response_root_key() {
        let resp = SubsonicResponse {
            inner: SubsonicBody {
                status: "ok",
                version: "1.16.1",
                server_type: "Subtidal",
                server_version: "0.1.0",
                open_subsonic: true,
                data: PingResponse {},
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.starts_with(r##"{"subsonic-response":"##));
    }

    #[test]
    fn open_subsonic_extensions_use_versions_array() {
        let resp = GetOpenSubsonicExtensionsResponse {
            extensions: vec![OpenSubsonicExtension {
                name: "transcodeOffset",
                versions: vec![1],
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r##"{"openSubsonicExtensions":[{"name":"transcodeOffset","versions":[1]}]}"##
        );
    }

    #[test]
    fn jukebox_control_serializes_status_and_playlist() {
        let resp = JukeboxControlResponse {
            status: JukeboxStatus {
                current_index: 7,
                playing: true,
                gain: 0.9,
                position: 67,
            },
            playlist: Some(JukeboxPlaylist { entry: vec![] }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r##"{"jukeboxStatus":{"currentIndex":7,"playing":true,"gain":0.9,"position":67},"jukeboxPlaylist":{"entry":[]}}"##
        );
    }
}
