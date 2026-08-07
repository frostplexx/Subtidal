// Stream metadata: playbackinfo manifest decoding. Backs the stream
// endpoint, which 302-redirects to a single-file Tidal CDN URL.
use base64::Engine;
use serde_json::Value;

use super::{Error, TidalClient, API_URL};

// Stream metadata for one track.
pub struct StreamInfo {
    pub mime_type: String,
    // Raw decoded manifest (MPD or BTS JSON). Unused for the 302 path;
    // the DASH->HLS rewrite reads it.
    #[allow(dead_code)]
    pub manifest: String,
    // Direct single-file URL when the manifest is playable as one file
    // (BTS). Segmented DASH (hi-res FLAC) has no such URL; the stream
    // handler falls back to a lower tier in that case.
    pub direct_url: Option<String>,
}

impl TidalClient {
    // Fetch playbackinfo for one track at the given quality tier.
    // Never cached: the CDN URLs carry short-lived signed tokens.
    pub async fn stream_info(&self, track_id: u64, quality: &str) -> Result<StreamInfo, Error> {
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
    let direct_url = if mime.contains("bts") {
        // BTS: JSON with a direct single-file URL.
        serde_json::from_str::<Value>(&manifest)
            .ok()
            .and_then(|v| v["urls"].get(0).and_then(|u| u.as_str()).map(String::from))
    } else {
        // DASH: segmented manifest (hi-res FLAC). No single-file URL;
        // a 302 cannot serve it. The stream handler falls back to HIGH.
        None
    };
    Ok(StreamInfo {
        mime_type: mime,
        manifest,
        direct_url,
    })
}
