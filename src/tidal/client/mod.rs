// Tidal API client: cached authenticated GETs plus per-entity methods.
// Auth (Authorization Code + PKCE login, token refresh, credential file) lives in
// auth.rs; each endpoint family gets its own module with an
// `impl TidalClient` block.
//   auth:   https://auth.tidal.com/v1/oauth2
//   api:    https://api.tidal.com/v1
//   stream: GET /tracks/{id}/playbackinfopostpaywall (v1; BTS single-file manifest)
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
mod jsonapi;
mod playlists;
mod radio;
mod search;
mod stream;
pub use stream::{Asset, SEGMENT_CONCURRENCY, StreamInfo};
use stream::StreamLimiter;
pub(crate) use playlists::ItemAddr;
mod tracks;
mod users;

pub use genres::{genre_albums, genre_key, genre_list, genre_tracks};

pub(crate) use feed::albums_from_page;
pub use favorites::FavoriteKind;

const AUTH_URL: &str = "https://auth.tidal.com/v1/oauth2";
const API_URL: &str = "https://api.tidal.com/v1";
const V2_URL: &str = "https://api.tidal.com/v2";
// OpenAPI (JSON:API) host, distinct from the legacy V2_URL above.
const OPENAPI_URL: &str = "https://openapi.tidal.com/v2";
const CLIENT_VERSION: &str = "2025.11.3";
const SCOPE: &str = "r_usr w_usr w_sub";
// Tidal caps a user's favorites list at 10,000 entries per kind.
pub const FAVORITES_CAP: u32 = 10_000;

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error),
    // Non-JSON API response: status plus the raw body, so a throttled
    // or misbehaving response stays diagnosable.
    HttpDecode(u16, String),
    Tidal(u16, String),
    Json(serde_json::Error),
    Auth(String),
    // A CDN asset whose bytes contradict its advertised size or shape (a segment shorter than its header claimed).  
    Malformed(String),
    RateLimited,
    NotLoggedIn,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // reqwest's Display stops at "error sending request"; the
            // cause chain (reset, refused, tls, dns) is what a log needs.
            Error::Http(e) => {
                write!(f, "http error: {e}")?;
                let mut source = std::error::Error::source(e);
                while let Some(s) = source {
                    write!(f, ": {s}")?;
                    source = s.source();
                }
                Ok(())
            }
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
            Error::Malformed(msg) => write!(f, "malformed asset: {msg}"),
            Error::RateLimited => write!(f, "stream limit exceeded"),
            Error::NotLoggedIn => {
                write!(
                    f,
                    "not logged in. run `subtidal login`, or open /setup on this server"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    // Rebuild an error handed back through a coalesced cache fetch: moka
    // shares one Arc<Error> between every waiter. Every variant but the
    // two non-Clone wrappers rebuilds exactly; those keep their message.
    fn shared(e: std::sync::Arc<Error>) -> Error {
        match &*e {
            Error::Http(inner) => Error::HttpDecode(0, format!("http error: {inner}")),
            Error::HttpDecode(s, b) => Error::HttpDecode(*s, b.clone()),
            Error::Tidal(s, b) => Error::Tidal(*s, b.clone()),
            Error::Json(inner) => Error::HttpDecode(0, format!("json error: {inner}")),
            Error::Auth(m) => Error::Auth(m.clone()),
            Error::Malformed(m) => Error::Malformed(m.clone()),
            Error::RateLimited => Error::RateLimited,
            Error::NotLoggedIn => Error::NotLoggedIn,
        }
    }

    // Tidal's `subStatus` from an error body, when it carried one.
    pub fn sub_status(&self) -> Option<u64> {
        match self {
            Error::Tidal(_, body) => serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| v.get("subStatus").and_then(|s| s.as_u64())),
            _ => None,
        }
    }

    // One short sentence for the client's error toast: what went wrong
    // and, when there is one, what the user can do about it. The full
    // diagnostic stays in the log.
    pub fn user_reason(&self) -> String {
        match self {
            Error::NotLoggedIn => "Subtidal is not logged into Tidal; open /setup".into(),
            Error::RateLimited => "too many streams at once; try again in a moment".into(),
            Error::Http(e) if e.is_timeout() => "Tidal did not answer in time".into(),
            Error::Http(e) if e.is_connect() => "could not reach Tidal".into(),
            Error::Http(_) => "the Tidal request failed".into(),
            Error::HttpDecode(429, _) => "Tidal is rate limiting this server; try again in a minute".into(),
            Error::HttpDecode(s, _) if (500..600).contains(s) => "Tidal is having trouble (server error)".into(),
            Error::HttpDecode(_, _) => "Tidal answered with something unreadable".into(),
            Error::Tidal(_, _) => match self.sub_status() {
                Some(4005) => "Tidal has not finished processing this track".into(),
                Some(4010) => "this Tidal account's monthly stream quota is exhausted".into(),
                Some(4032) | Some(4035) => "not available in your region on Tidal".into(),
                Some(4033) => "needs a higher Tidal subscription tier".into(),
                _ => match self {
                    Error::Tidal(401, _) | Error::Tidal(403, _) => {
                        "Tidal rejected the session; open /setup to sign in again".into()
                    }
                    Error::Tidal(404, _) => "not found on Tidal".into(),
                    Error::Tidal(429, _) => "Tidal is rate limiting this server; try again in a minute".into(),
                    Error::Tidal(s, _) if (500..600).contains(s) => "Tidal is having trouble (server error)".into(),
                    Error::Tidal(s, _) => format!("Tidal answered {s}"),
                    _ => unreachable!(),
                },
            },
            Error::Json(_) => "Tidal answered with something unreadable".into(),
            Error::Auth(m) => m.clone(),
            Error::Malformed(_) => "Tidal sent a broken audio segment".into(),
        }
    }

    // True when Tidal refuses the track itself: subStatus 4005 ("Asset
    // is not ready for playback"). The asset is not playable for this
    // account; no retry can change that, and it is not throttle evidence.
    pub fn is_unavailable_asset(&self) -> bool {
        self.sub_status() == Some(4005)
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
    // PKCE verifier for a login that has been started but not yet
    // redeemed. The web login spans two requests, so the verifier cannot
    // live on the stack the way the CLI prompt kept it.
    pending_login: Mutex<Option<auth::PendingLogin>>,
    meta_cache: Cache<String, Value>,
    search_cache: Cache<String, Value>,
    mix_cache: Cache<String, Value>,
    playlist_cache: Cache<String, Value>,
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
        // Bounded so a hung Tidal or CDN request cannot pin a client
        // request (or a stream-limiter permit) forever. The total covers
        // one segment or one API page; nothing here streams a whole track
        // through a single request.
        let http = reqwest::Client::builder()
            .user_agent(
                "AppleCoreMedia/1.0.0.24A5408d (iPhone; U; CPU OS 27_0 like Mac OS X; en_us)",
            )
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
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
            pending_login: Mutex::new(None),
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
        let url = format!("{base}{full}");
        fetch_coalesced(cache, full, &self.http, url, token).await
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
        self.get_json_base(base, &format!("{path}?{}", encode_query(params)), cache)
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
        let token = self.access_token().await?;
        let url = format!("{OPENAPI_URL}{full_path}");
        fetch_coalesced(cache, full_path.to_string(), &self.http, url, token).await
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

// A cached, authenticated GET with in-flight coalescing: concurrent
// misses on one key share a single fetch (a client firing getArtists,
// getIndexes and getStarred at startup would otherwise walk the same
// favorites three times). Errors are never cached. The official client
// sends x-tidal-client-version on every call; the v2 API rejects
// requests without it (400 subStatus 1002), v1 tolerates it.
// Boxed: the shared-fetch future would otherwise deepen every caller's
// state machine past the trait solver's recursion limit.
fn fetch_coalesced<'a>(
    cache: &'a Cache<String, Value>,
    key: String,
    http: &reqwest::Client,
    url: String,
    token: String,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, Error>> + Send + 'a>> {
    let http = http.clone();
    Box::pin(async move {
    cache
        .try_get_with(key, async move {
            let resp = http
                .get(url)
                .bearer_auth(token)
                .header("x-tidal-client-version", CLIENT_VERSION)
                .send()
                .await?;
            let status = resp.status();
            let body: Value = resp.json().await?;
            if !status.is_success() {
                return Err(Error::Tidal(status.as_u16(), body.to_string()));
            }
            Ok(body)
        })
        .await
        .map_err(Error::shared)
    })
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

// `k=v&..` with keys and values percent-encoded, via reqwest's Url so
// the encoding matches what the client would send itself.
pub(crate) fn encode_query<K: AsRef<str>, V: AsRef<str>>(params: &[(K, V)]) -> String {
    let mut url = reqwest::Url::parse("http://x/").expect("static URL");
    url.query_pairs_mut().extend_pairs(params.iter().map(|(k, v)| (k.as_ref(), v.as_ref())));
    url.query().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::encode_query;

    #[test]
    fn encode_query_escapes_reserved_characters() {
        assert_eq!(
            encode_query(&[("query", "Alcest & Amesoeurs"), ("limit", "5")]),
            "query=Alcest+%26+Amesoeurs&limit=5"
        );
        assert_eq!(encode_query::<&str, &str>(&[]), "");
        assert_eq!(encode_query(&[("q", String::from("ü/é"))]), "q=%C3%BC%2F%C3%A9");
    }
}

#[cfg(test)]
mod coalesce_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Concurrent misses on one key must produce one upstream request.
    // The URL points at a closed local port so the fetch fails fast;
    // what matters is how many futures ran, which the counter sees
    // through a wrapper cache key.
    #[tokio::test]
    async fn concurrent_misses_share_one_fetch() {
        let cache: Cache<String, Value> = Cache::builder().build();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let cache = cache.clone();
            let hits = hits.clone();
            tasks.push(tokio::spawn(async move {
                cache
                    .try_get_with("k".to_string(), async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        Ok::<Value, Error>(serde_json::json!(1))
                    })
                    .await
                    .map_err(Error::shared)
            }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap().unwrap(), serde_json::json!(1));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn shared_errors_keep_their_status() {
        let e = Error::shared(std::sync::Arc::new(Error::Tidal(404, "gone".into())));
        assert!(matches!(e, Error::Tidal(404, ref b) if b == "gone"));
        assert!(matches!(
            Error::shared(std::sync::Arc::new(Error::NotLoggedIn)),
            Error::NotLoggedIn
        ));
    }
}

impl TidalClient {
    // Pre-fetch what a client asks for first (the library index, starred
    // lists, playlists, genres, the home feed) so the first requests
    // after a restart hit warm caches instead of Tidal. Best-effort and
    // concurrent; a failure only logs, the request path refetches.
    pub async fn warm_caches(client: &'static TidalClient) {
        let started = std::time::Instant::now();
        let (tracks, albums, artists, playlists, mixes, genres, feed) = tokio::join!(
            TidalClient::favorite_tracks_parallel(client),
            TidalClient::favorite_albums(client, 0, FAVORITES_CAP),
            TidalClient::favorite_artists(client, 0, FAVORITES_CAP),
            client.user_playlists(0, 500),
            client.my_mixes(),
            genre_list(client),
            client.home_feed("static"),
        );
        for (name, failed) in [
            ("favorite tracks", tracks.is_err()),
            ("favorite albums", albums.is_err()),
            ("favorite artists", artists.is_err()),
            ("playlists", playlists.is_err()),
            ("mixes", mixes.is_err()),
            ("genres", genres.is_err()),
            ("home feed", feed.is_err()),
        ] {
            if failed {
                tracing::debug!("cache warm-up: {name} fetch failed");
            }
        }
        tracing::info!("caches warmed in {:.1?}", started.elapsed());
    }
}

#[cfg(test)]
mod reason_tests {
    use super::Error;

    #[test]
    fn user_reasons_name_the_actionable_cause() {
        let sub = |s: u64| Error::Tidal(401, format!(r#"{{"status":401,"subStatus":{s}}}"#));
        assert!(sub(4010).user_reason().contains("quota"));
        assert!(sub(4032).user_reason().contains("region"));
        assert!(sub(4033).user_reason().contains("subscription"));
        assert!(Error::Tidal(401, "{}".into()).user_reason().contains("/setup"));
        assert!(Error::Tidal(429, "{}".into()).user_reason().contains("rate limiting"));
        assert!(Error::Tidal(404, "{}".into()).user_reason().contains("not found"));
        assert!(Error::HttpDecode(503, "<html>".into()).user_reason().contains("server error"));
        assert!(Error::NotLoggedIn.user_reason().contains("/setup"));
        assert!(Error::RateLimited.user_reason().contains("try again"));
    }
}
