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
