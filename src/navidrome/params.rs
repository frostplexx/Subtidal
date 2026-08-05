use serde::Deserialize;

// Query params shared by /rest/* endpoints. All optional at parse time;
// per-endpoint and auth requirements are enforced in handlers/auth.
#[derive(Deserialize)]
pub struct QueryParams {
    pub u: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub p: Option<String>,
    #[allow(dead_code)]
    pub v: Option<String>,
    #[allow(dead_code)]
    pub c: Option<String>,
    // search3
    pub query: Option<String>,    #[serde(rename = "artistCount")]
    pub artist_count: Option<u32>,
    #[serde(rename = "artistOffset")]
    pub artist_offset: Option<u32>,
    #[serde(rename = "albumCount")]
    pub album_count: Option<u32>,
    #[serde(rename = "albumOffset")]
    pub album_offset: Option<u32>,
    #[serde(rename = "songCount")]
    pub song_count: Option<u32>,
    #[serde(rename = "songOffset")]
    pub song_offset: Option<u32>,
    // getCoverArt
    pub id: Option<String>,
    pub size: Option<u32>,
}
