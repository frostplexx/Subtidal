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

impl QueryParams {
    // Merge the URL query string and a form-encoded body, then parse.
    // Subsonic clients send params either in the URL (GET) or in the
    // body (POST, the OpenSubsonic formPost extension).
    // Concatenate the URL query string and a form-encoded body into one
    // urlencoded string. Clients send params in the URL (GET) or in the
    // body (POST, the OpenSubsonic formPost extension), rarely both.
    pub fn merge_raw(query: &str, body: &[u8]) -> String {
        let body = std::str::from_utf8(body).unwrap_or("");
        match (query.is_empty(), body.is_empty()) {
            (_, true) => query.to_owned(),
            (true, false) => body.to_owned(),
            (false, false) => format!("{query}&{body}"),
        }
    }

    pub fn from_merged(merged: &str) -> Result<Self, serde_urlencoded::de::Error> {
        serde_urlencoded::from_str(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(query: &str, body: &[u8]) -> Result<QueryParams, serde_urlencoded::de::Error> {
        QueryParams::from_merged(&QueryParams::merge_raw(query, body))
    }

    #[test]
    fn parses_query_string() {
        let p = parse("u=admin&v=1.16.1&c=curl", b"").unwrap();
        assert_eq!(p.u.as_deref(), Some("admin"));
        assert_eq!(p.v.as_deref(), Some("1.16.1"));
    }

    #[test]
    fn parses_form_body() {
        let p = parse("", b"u=admin&v=1.16.1&c=curl").unwrap();
        assert_eq!(p.u.as_deref(), Some("admin"));
    }

    #[test]
    fn merges_query_and_body() {
        let p = parse("u=admin", b"query=abc").unwrap();
        assert_eq!(p.u.as_deref(), Some("admin"));
        assert_eq!(p.query.as_deref(), Some("abc"));
    }

    #[test]
    fn empty_input_yields_empty_params() {
        let p = parse("", b"").unwrap();
        assert!(p.u.is_none() && p.query.is_none());
    }

    #[test]
    fn rejects_type_mismatch() {
        // size is u32; a non-numeric value must fail to parse.
        assert!(parse("size=abc", b"").is_err());
    }
}
