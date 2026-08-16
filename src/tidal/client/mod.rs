// Tidal API client: cached authenticated GETs plus per-entity methods.
// Auth (device-code login, token refresh, keyring) lives in auth.rs; each
// endpoint family gets its own module with an `impl TidalClient` block.
//   auth:   https://auth.tidal.com/v1/oauth2
//   api:    https://api.tidal.com/v1
//   stream: GET /tracks/{id}/playbackinfopostpaywall
use std::time::Duration;

use moka::future::Cache;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::settings::Settings;

use super::embedded;

mod albums;
mod artists;
mod auth;
mod favorites;
mod feed;
mod playlists;
mod search;
mod stream;
pub use stream::DashInfo;
use stream::StreamLimiter;
// Test fixtures construct Segment values directly.
#[cfg(test)]
pub(crate) use stream::Segment;
mod tracks;
mod users;

pub(crate) use feed::albums_from_page;
pub use favorites::FavoriteKind;

const AUTH_URL: &str = "https://auth.tidal.com/v1/oauth2";
const API_URL: &str = "https://api.tidal.com/v1";
const V2_URL: &str = "https://api.tidal.com/v2";
// OpenAPI (JSON:API) host, distinct from the legacy V2_URL above.
const OPENAPI_URL: &str = "https://openapi.tidal.com/v2";
const CLIENT_VERSION: &str = "2025.11.3";
const KEYRING_SERVICE: &str = "Subtidal";
const KEYRING_USER: &str = "tidal";
const SCOPE: &str = "r_usr w_usr w_sub";

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error),
    Tidal(u16, String),
    Json(serde_json::Error),
    Auth(String),
    RateLimited,
    NotLoggedIn,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Http(e) => write!(f, "http error: {e}"),
            Error::Tidal(code, body) => write!(f, "tidal api error {code}: {body}"),
            Error::Json(e) => write!(f, "json error: {e}"),
            Error::Auth(msg) => write!(f, "auth error: {msg}"),
            Error::RateLimited => write!(f, "stream limit exceeded"),
            Error::NotLoggedIn => {
                write!(f, "not logged in. run `subtidal login` first")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

pub struct TidalClient {
    http: reqwest::Client,
    client_id: String,
    client_secret: Option<String>,
    tokens: Mutex<Option<auth::Tokens>>,
    meta_cache: Cache<String, Value>,
    search_cache: Cache<String, Value>,
    mix_cache: Cache<String, Value>,
    playlist_cache: Cache<String, Value>,
    // Hard cap on parallel playbackinfo fetches and on stream starts
    // per window; see stream.rs. Excess stream requests are rejected.
    stream_limiter: StreamLimiter,
}

impl TidalClient {
    pub fn new(settings: &Settings) -> Self {
        // Present as a native media client, not a script. Tidal's edge
        // classifies requests by client shape and grants its own app a far
        // larger rate budget; the UA below is what the official iOS app
        // sends on every stream request (captured in a HAR of an in-app
        // download).
        let http = reqwest::Client::builder()
            .user_agent(
                "AppleCoreMedia/1.0.0.24A5408d (iPhone; U; CPU OS 27_0 like Mac OS X; en_us)",
            )
            .build()
            .expect("failed to build reqwest client");
        let (client_id, client_secret) = match &settings.tidal_client_id {
            Some(id) if !id.is_empty() => (id.clone(), settings.tidal_client_secret.clone()),
            _ => (embedded::client_id(), Some(embedded::client_secret())),
        };
        Self {
            http,
            client_id,
            client_secret,
            tokens: Mutex::new(None),
            meta_cache: Cache::builder()
                .time_to_live(Duration::from_secs(6 * 3600))
                .max_capacity(10_000)
                .support_invalidation_closures()
                .build(),
            search_cache: Cache::builder()
                .time_to_live(Duration::from_secs(300))
                .max_capacity(1_000)
                .build(),
            // Mixes regenerate, so their pages and items never enter the
            // 6h meta_cache; five minutes keeps them near-fresh.
            mix_cache: Cache::builder()
                .time_to_live(Duration::from_secs(300))
                .max_capacity(1_000)
                .build(),
            // Playlists can change from other clients, so their list and
            // pages live in a short cache, not the 6h meta_cache. Subtidal's
            // own mutations invalidate it immediately.
            playlist_cache: Cache::builder()
                .time_to_live(Duration::from_secs(300))
                .max_capacity(10_000)
                .support_invalidation_closures()
                .build(),
            stream_limiter: StreamLimiter::new(),
        }
    }

    // Cached authenticated GET. Errors and non-2xx are never cached.
    // countryCode is required by most Tidal v1 endpoints, so it goes on every
    // request. The cache key includes it, keeping the lookup consistent.
    async fn get_json(&self, path: &str, cache: &Cache<String, Value>) -> Result<Value, Error> {
        self.get_json_base(API_URL, path, cache).await
    }

    // v2 endpoints require the client-version header; get_json_base adds it
    // whenever the base URL contains "/v2/". get_json_q_v2 passes params
    // and is the only v2 entry point in use.
    async fn get_json_base(
        &self,
        base: &str,
        path: &str,
        cache: &Cache<String, Value>,
    ) -> Result<Value, Error> {
        let token = self.access_token().await?;
        let mut full = path.to_string();
        if let Some(cc) = self.country_code().await? {
            full.push_str(if full.contains('?') { "&" } else { "?" });
            full.push_str(&format!("countryCode={cc}"));
        }
        if let Some(v) = cache.get(&full).await {
            return Ok(v);
        }
        // The official client sends x-tidal-client-version on every API
        // call. The v2 API rejects requests without it (400 subStatus
        // 1002); v1 tolerates it and the stream fetches expect it.
        let req = self
            .http
            .get(format!("{base}{full}"))
            .bearer_auth(token)
            .header("x-tidal-client-version", CLIENT_VERSION);
        let resp = req.send().await?;
        let status = resp.status();
        let body: Value = resp.json().await?;
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), body.to_string()));
        }
        cache.insert(full, body.clone()).await;
        Ok(body)
    }

    // Authenticated GET with url-encoded query params. get_json appends
    // countryCode and handles caching; the cache key covers the full query,
    // so distinct params never collide.
    async fn get_json_q(
        &self,
        path: &str,
        params: &[(&str, &str)],
        cache: &Cache<String, Value>,
    ) -> Result<Value, Error> {
        self.get_json_q_base(API_URL, path, params, cache).await
    }

    async fn get_json_q_v2(
        &self,
        path: &str,
        params: &[(&str, &str)],
        cache: &Cache<String, Value>,
    ) -> Result<Value, Error> {
        self.get_json_q_base(V2_URL, path, params, cache).await
    }

    async fn get_json_q_base(
        &self,
        base: &str,
        path: &str,
        params: &[(&str, &str)],
        cache: &Cache<String, Value>,
    ) -> Result<Value, Error> {
        if params.is_empty() {
            return self.get_json_base(base, path, cache).await;
        }
        let query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, percent_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        self.get_json_base(base, &format!("{path}?{query}"), cache).await
    }
}

// Minimal percent-encoding for query strings. No new dependency needed.
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
