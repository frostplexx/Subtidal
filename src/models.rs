use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Post {
    pub id: u64,
    pub title: String,
    pub body: String,
}

// Common envelope for all subsonic-response payloads
#[derive(Serialize)]
pub struct SubsonicResponse<T: Serialize> {
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
