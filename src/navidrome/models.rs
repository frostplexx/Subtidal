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

// search3 data: { searchResult3: { artist, album, song } }
#[derive(Serialize)]
pub struct SearchResult3Response {
    #[serde(rename = "searchResult3")]
    pub search_result: SearchResult3,
}

#[derive(Serialize)]
pub struct SearchResult3 {
    pub artist: Vec<ArtistId3>,
    pub album: Vec<AlbumId3>,
    pub song: Vec<Child>,
}

// Subsonic child (song entry). Optional fields omitted when unknown.
#[derive(Serialize)]
pub struct Child {
    pub id: String,
    #[serde(rename = "parent")]
    pub parent: String,
    #[serde(rename = "isDir")]
    pub is_dir: bool,
    pub title: String,
    pub album: String,
    pub artist: String,
    #[serde(rename = "track")]
    pub track: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    pub duration: u32,
    #[serde(rename = "discNumber", skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u32>,
    #[serde(rename = "albumId")]
    pub album_id: String,
    #[serde(rename = "artistId")]
    pub artist_id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Serialize)]
pub struct AlbumId3 {
    pub id: String,
    pub name: String,
    pub artist: String,
    #[serde(rename = "artistId")]
    pub artist_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(rename = "songCount", skip_serializing_if = "Option::is_none")]
    pub song_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
}

#[derive(Serialize)]
pub struct ArtistId3 {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(rename = "albumCount", skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u32>,
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
}
