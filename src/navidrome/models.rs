use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Post {
    pub id: u64,
    pub title: String,
    pub body: String,
}

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
    pub username: &'static str,
    pub email: &'static str,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_response_has_subsonic_response_root_key() {
        let resp = SubsonicResponse {
            inner: SubsonicBody {
                status: "ok",
                version: "1.16.1",
                server_type: "HighTide",
                server_version: "0.1.0",
                open_subsonic: true,
                data: PingResponse {},
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.starts_with(r##"{"subsonic-response":"##));
    }
}
