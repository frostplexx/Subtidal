// Stream metadata: playbackinfo manifest decoding. Backs the stream
// endpoint, which is not wired yet.
use base64::Engine;
use serde_json::Value;

use super::{Error, TidalClient, API_URL};

// Stream metadata for one track.
#[allow(dead_code)]
pub struct StreamInfo {
    pub mime_type: String,
    pub manifest: String,
    pub direct_url: Option<String>,
}

impl TidalClient {
    #[allow(dead_code)]
    pub async fn stream_url(&self, track_id: u64, quality: &str) -> Result<StreamInfo, Error> {
        let token = self.access_token().await?;
        let cc = self.country_code().await?;
        let mut query = vec![
            ("audioquality", quality),
            ("playbackmode", "STREAM"),
            ("assetpresentation", "FULL"),
        ];
        if let Some(cc) = &cc {
            query.push(("countryCode", cc.as_str()));
        }
        let resp = self
            .http
            .get(format!("{API_URL}/tracks/{track_id}/playbackinfopostpaywall"))
            .bearer_auth(token)
            .query(&query)
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await?;
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), body.to_string()));
        }
        parse_stream(body)
    }
}

// Decode a playbackinfo manifest into StreamInfo.
#[allow(dead_code)]
fn parse_stream(body: Value) -> Result<StreamInfo, Error> {
    let mime = body["manifestMimeType"].as_str().unwrap_or("").to_string();
    let manifest_b64 = body["manifest"]
        .as_str()
        .ok_or_else(|| Error::Auth("response missing manifest".into()))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(manifest_b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(manifest_b64))
        .map_err(|e| Error::Auth(format!("manifest decode failed: {e}")))?;
    let manifest = String::from_utf8_lossy(&decoded).into_owned();
    let direct_url = if mime.contains("dash") {
        extract_dash_url(&manifest)
    } else if mime.contains("bts") {
        serde_json::from_str::<Value>(&manifest)
            .ok()
            .and_then(|v| v["url"].as_str().map(|s| s.to_string()))
    } else {
        None
    };
    Ok(StreamInfo {
        mime_type: mime,
        manifest,
        direct_url,
    })
}

// First <BaseURL> element. TODO: full MPD parsing for segment templates.
#[allow(dead_code)]
fn extract_dash_url(manifest: &str) -> Option<String> {
    const OPEN: &str = "<BaseURL>";
    const CLOSE: &str = "</BaseURL>";
    let start = manifest.find(OPEN)? + OPEN.len();
    let end = manifest[start..].find(CLOSE)? + start;
    let url = manifest[start..end].trim();
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}
