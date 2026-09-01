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
mod genres;
mod jsonapi;mod playlists;
mod playqueues;
mod search;
mod stream;
pub use stream::DashInfo;
use stream::StreamLimiter;
// Test fixtures construct Segment values directly.
#[cfg(test)]
pub(crate) use stream::Segment;
pub(crate) use playlists::ItemAddr;
mod tracks;
mod users;

pub use genres::genre_list;

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
    // Non-JSON API response: status plus the raw body, so a throttled
    // or misbehaving response stays diagnosable.
    HttpDecode(u16, String),
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
            Error::HttpDecode(status, body) => {
                if body.trim().is_empty() {
                    write!(f, "tidal answered {status} with an empty body")
                } else {
                    // A short preview keeps the log line readable.
                    let preview: String = body.chars().take(300).collect();
                    write!(f, "tidal answered {status} with a non-JSON body: {preview}")
                }
            }
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

impl Error {
    // True when Tidal refuses the track itself: subStatus 4005 ("Asset
    // is not ready for playback"). The asset is not playable for this
    // account; no retry can change that, and it is not throttle evidence.
    pub fn is_unavailable_asset(&self) -> bool {
        match self {
            Error::Tidal(_, body) => serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("subStatus").and_then(|s| s.as_u64()))
                == Some(4005),
            _ => false,
        }
    }
}

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
    // Play queue reads are mirrored onto Tidal's own queue, which the
    // mobile clients churn constantly; a minute of staleness is fine.
    queue_cache: Cache<String, Value>,
    // The user's play queue id, resolved once and reused until the
    // queue is deleted.
    queue_id: Mutex<Option<String>>,
    // Hard cap on parallel playbackinfo fetches; see stream.rs. A wait
    // that exceeds the slot bound is rejected with RateLimited.
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
            queue_cache: Cache::builder()
                .time_to_live(Duration::from_secs(60))
                .max_capacity(100)
                .support_invalidation_closures()
                .build(),
            queue_id: Mutex::new(None),
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
        self.get_json_base(base, &format!("{path}?{query}"), cache)
            .await
    }

    // --- OpenAPI (JSON:API) helpers --------------------------------
    // The v2 host differs from v1 in three ways: no countryCode query
    // param (it derives from the token; sending one errors), the
    // mandatory x-tidal-client-version header, and query values that
    // keep commas and brackets literal (the official SDK serializes
    // with allowReserved, so `filter[query]` keys and comma-joined
    // include lists pass through raw).
    async fn openapi_get(
        &self,
        path: &str,
        params: &[(&str, &str)],
        cache: &Cache<String, Value>,
    ) -> Result<Value, Error> {
        let query: String = if params.is_empty() {
            String::new()
        } else {
            let q = params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            format!("?{q}")
        };
        self.openapi_get_raw(&format!("{path}{query}"), cache).await
    }

    // GET with an already-fully-formed path+query string. Pagination
    // cursors come back opaque and must re-send verbatim, so walkers
    // append them here rather than through percent-encoding.
    async fn openapi_get_raw(
        &self,
        full_path: &str,
        cache: &Cache<String, Value>,
    ) -> Result<Value, Error> {
        if let Some(v) = cache.get(full_path).await {
            return Ok(v);
        }
        let token = self.access_token().await?;
        let resp = self
            .http
            .get(format!("{OPENAPI_URL}{full_path}"))
            .bearer_auth(token)
            .header("x-tidal-client-version", CLIENT_VERSION)
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await?;
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), body.to_string()));
        }
        cache.insert(full_path.to_string(), body.clone()).await;
        Ok(body)
    }

    // Mutating JSON:API request (POST/PATCH/DELETE). Never cached.
    // JSON:API errors read the `errors` array for the message.
    async fn openapi_send(
        &self,
        method: reqwest::Method,
        path: &str,
        payload: Option<&Value>,
    ) -> Result<Value, Error> {
        let token = self.access_token().await?;
        let mut req = self
            .http
            .request(method, format!("{OPENAPI_URL}{path}"))
            .bearer_auth(token)
            .header("x-tidal-client-version", CLIENT_VERSION);
        req = match payload {
            Some(body) => req
                .header("content-type", "application/vnd.api+json")
                .json(body),
            None => req,
        };
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        let body: Value = serde_json::from_str(&text).unwrap_or(serde_json::json!(null));
        if !status.is_success() {
            // JSON:API errors read prettier than the raw document.
            let message = body["errors"][0]["detail"]
                .as_str()
                .or_else(|| body["errors"][0]["title"].as_str())
                .map(String::from)
                .unwrap_or(text);
            return Err(Error::Tidal(status.as_u16(), message));
        }
        Ok(body)
    }
}

// Fetch one offset-paged page from a legacy v1 items endpoint.
async fn v1_page(
    client: &'static TidalClient,
    path: &str,
    cache: &'static Cache<String, Value>,
    extra: &'static [(&'static str, &'static str)],
    offset: u32,
    limit: u32,
) -> Result<Value, Error> {
    let offset = offset.to_string();
    let limit = limit.to_string();
    let mut params: Vec<(&str, &str)> = Vec::with_capacity(extra.len() + 2);
    params.extend_from_slice(extra);
    params.push(("limit", limit.as_str()));
    params.push(("offset", offset.as_str()));
    client.get_json_q(path, &params, cache).await
}

// All offset pages of a legacy v1 items list, fetched concurrently and
// reordered by offset on return. Page 0 learns totalNumberOfItems; a
// missing total degrades to a sequential walk until a short page. The
// v1 item lists carry replayGain/peak on every track and offset paging,
// which the v2 relationships never do.
pub(crate) async fn v1_pages_parallel(
    client: &'static TidalClient,
    path: &str,
    cache: &'static Cache<String, Value>,
    extra: &'static [(&'static str, &'static str)],
    page_size: u32,
    in_flight: usize,
) -> Result<Vec<Value>, Error> {
    let first = v1_page(client, path, cache, extra, 0, page_size).await?;
    let mut items: Vec<Value> = first["items"].as_array().cloned().unwrap_or_default();
    let total = match first["totalNumberOfItems"].as_u64() {
        Some(t) => t,
        None => {
            let mut offset = page_size;
            loop {
                let page = v1_page(client, path, cache, extra, offset, page_size).await?;
                let batch = page["items"].as_array().cloned().unwrap_or_default();
                let n = batch.len();
                items.extend(batch);
                offset += page_size;
                if n < page_size as usize || items.len() >= 10_000 {
                    break;
                }
            }
            return Ok(items);
        }
    };
    let mut offset = page_size;
    while offset < total as u32 {
        let end = (offset as u64 + in_flight as u64 * page_size as u64).min(total);
        let mut handles = Vec::with_capacity(in_flight);
        for off in (offset..end as u32).step_by(page_size as usize) {
            let path = path.to_string();
            handles.push(tokio::spawn(async move {
                v1_page(client, &path, cache, extra, off, page_size).await
            }));
        }
        for handle in handles {
            let page = handle
                .await
                .map_err(|e| Error::HttpDecode(500, format!("v1 page task failed: {e}")))??;
            items.extend(page["items"].as_array().cloned().unwrap_or_default());
        }
        offset = end as u32;
    }
    Ok(items)
}

// Offset pages walked sequentially, stopping once limit items are in
// hand or the server returns a short page. For bounded prefixes where
// fetching everything would waste requests (top tracks).
pub(crate) async fn v1_prefix(
    client: &'static TidalClient,
    path: &str,
    cache: &'static Cache<String, Value>,
    extra: &'static [(&'static str, &'static str)],
    page_size: u32,
    limit: u32,
) -> Result<Vec<Value>, Error> {
    let mut items: Vec<Value> = Vec::new();
    let mut offset = 0u32;
    loop {
        let page = v1_page(client, path, cache, extra, offset, page_size).await?;
        let batch = page["items"].as_array().cloned().unwrap_or_default();
        let n = batch.len();
        items.extend(batch);
        offset += page_size;
        if items.len() >= limit as usize || n < page_size as usize {
            break;
        }
    }
    items.truncate(limit as usize);
    Ok(items)
}

// Minimal percent-encoding for query strings. No new dependency needed.
pub(crate) fn percent_encode(s: &str) -> String {
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
