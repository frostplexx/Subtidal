// Tidal API client: device-code login, token refresh, cached GETs, streams.
// Endpoints and flows mirror the working sone client:
//   auth:   https://auth.tidal.com/v1/oauth2
//   api:    https://api.tidal.com/v1
//   stream: GET /tracks/{id}/playbackinfopostpaywall
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use keyring::{Entry, Error as KeyringError};
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::settings::Settings;

use super::embedded;

const AUTH_URL: &str = "https://auth.tidal.com/v1/oauth2";
const API_URL: &str = "https://api.tidal.com/v1";
const KEYRING_SERVICE: &str = "Subtidal";
const KEYRING_USER: &str = "tidal";
const SCOPE: &str = "r_usr w_usr w_sub";

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error),
    Tidal(u16, String),
    Json(serde_json::Error),
    Auth(String),
    NotLoggedIn,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Http(e) => write!(f, "http error: {e}"),
            Error::Tidal(code, body) => write!(f, "tidal api error {code}: {body}"),
            Error::Json(e) => write!(f, "json error: {e}"),
            Error::Auth(msg) => write!(f, "auth error: {msg}"),
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuth {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct AuthTokens {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    user_id: u64,
    #[serde(default)]
    country_code: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    expires_at: u64, // unix seconds
    user_id: Option<u64>,
    country_code: Option<String>,
}

impl Tokens {
    fn expired(&self, now: u64) -> bool {
        self.expires_at.saturating_sub(60) <= now
    }
}

pub struct StreamInfo {
    pub mime_type: String,
    pub manifest: String,
    pub direct_url: Option<String>,
}

pub struct TidalClient {
    http: reqwest::Client,
    client_id: String,
    client_secret: Option<String>,
    tokens: Mutex<Option<Tokens>>,
    meta_cache: Cache<String, Value>,
    search_cache: Cache<String, Value>,
}

impl TidalClient {
    pub fn new(settings: &Settings) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("Subtidal/0.1")
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
                .build(),
            search_cache: Cache::builder()
                .time_to_live(Duration::from_secs(300))
                .max_capacity(1_000)
                .build(),
        }
    }

    // Add client_id (and client_secret when set) to an auth form body.
    // Returns a fresh Vec of (name, value) pairs owned by the caller.
    fn auth_form(&self, params: Vec<(&str, &str)>) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = params
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        out.push(("client_id".to_string(), self.client_id.clone()));
        if let Some(secret) = &self.client_secret {
            out.push(("client_secret".to_string(), secret.clone()));
        }
        out
    }

    // Tokens live in the OS keyring (macOS Keychain). Set SUBTIDAL_TOKEN_FILE
    // during development to store them in a file instead, skipping the
    // Keychain prompt that repeats on every rebuild.
    fn token_file_override() -> Option<std::path::PathBuf> {
        std::env::var_os("SUBTIDAL_TOKEN_FILE").map(std::path::PathBuf::from)
    }

    fn keyring_entry(&self) -> Result<Entry, Error> {
        Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .map_err(|e| Error::Auth(format!("keyring init failed: {e}")))
    }

    fn store_tokens(&self, tokens: &Tokens) -> Result<(), Error> {
        let json = serde_json::to_string(tokens)?;
        if let Some(path) = Self::token_file_override() {
            std::fs::write(&path, &json)
                .and_then(|_| {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                })
                .map_err(|e| Error::Auth(format!("token file write failed: {e}")))?;
            return Ok(());
        }
        self.keyring_entry()?
            .set_password(&json)
            .map_err(|e| Error::Auth(format!("keyring set failed: {e}")))
    }

    fn load_tokens(&self) -> Result<Option<Tokens>, Error> {
        if let Some(path) = Self::token_file_override() {
            let json = match std::fs::read_to_string(&path) {
                Ok(j) => j,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(Error::Auth(format!("token file read failed: {e}"))),
            };
            return Ok(Some(serde_json::from_str(&json)?));
        }
        match self.keyring_entry()?.get_password() {
            Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(Error::Auth(format!("keyring get failed: {e}"))),
        }
    }

    // Device-code login. Prints the code for the CLI, polls until the user
    // authorizes, then persists the tokens to the OS keyring.
    pub async fn login(&self) -> Result<(), Error> {
        if self.client_id.starts_with("REPLACE_") {
            return Err(Error::Auth(
                "Tidal credentials are not configured. Run:\n  \
                 python3 scripts/gen_embedded.py CLIENT_ID CLIENT_SECRET > src/tidal/embedded.rs\n\
                 then rebuild"
                    .into(),
            ));
        }

        let resp = self
            .http
            .post(format!("{AUTH_URL}/device_authorization"))
            .form(&self.auth_form(vec![("scope", SCOPE)]))
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            if body.contains("not a Limited Input Device client") {
                return Err(Error::Auth(
                    "this client_id does not support the device-code flow. \
                     use credentials from the native Android app, not the web player"
                        .into(),
                ));
            }
            return Err(Error::Tidal(status.as_u16(), body));
        }
        // Tidal returns HTTP 200 with an error payload for some failures.
        if let Ok(v) = serde_json::from_str::<Value>(&body) {
            if v.get("sub_status").is_some() || v.get("error").is_some() {
                if v["sub_status"] == 1002 {
                    return Err(Error::Auth(
                        "this client_id does not support the device-code flow. \
                         use credentials from the native Android app, not the web player"
                            .into(),
                    ));
                }
                let msg = v["errorDescription"]
                    .as_str()
                    .or_else(|| v["error"].as_str())
                    .unwrap_or("unknown error");
                return Err(Error::Auth(format!(
                    "device authorization refused: {msg}"
                )));
            }
        }
        let auth: DeviceAuth = serde_json::from_str(&body)?;

        println!("Open {}/{} in a browser", auth.verification_uri, auth.user_code);

        let deadline = Instant::now() + Duration::from_secs(auth.expires_in);
        loop {
            if Instant::now() >= deadline {
                return Err(Error::Auth("device authorization timed out".into()));
            }
            let resp = self
                .http
                .post(format!("{AUTH_URL}/token"))
                .form(&self.auth_form(vec![
                    ("device_code", &auth.device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("scope", SCOPE),
                ]))
                .send()
                .await?;
            let status = resp.status();
            let body = resp.text().await?;
            if status.as_u16() == 400
                && (body.contains("authorization_pending") || body.contains("slow_down"))
            {
                tokio::time::sleep(Duration::from_secs(auth.interval)).await;
                continue;
            }
            if !status.is_success() {
                return Err(Error::Tidal(status.as_u16(), body));
            }

            let auth_tokens: AuthTokens = serde_json::from_str(&body)?;
            let session = self.session_with(&auth_tokens.access_token).await?;
            let tokens = Tokens {
                access_token: auth_tokens.access_token,
                refresh_token: auth_tokens.refresh_token.unwrap_or_default(),
                expires_at: unix_now() + auth_tokens.expires_in,
                user_id: Some(session.user_id),
                country_code: session.country_code,
            };
            self.store_tokens(&tokens)?;
            println!(
                "Logged in. user_id={} country={:?}",
                tokens.user_id.unwrap_or(0),
                tokens.country_code
            );
            return Ok(());
        }
    }

    async fn session_with(&self, access_token: &str) -> Result<Session, Error> {
        let resp = self
            .http
            .get(format!("{API_URL}/sessions"))
            .bearer_auth(access_token)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), body));
        }
        Ok(serde_json::from_str(&body)?)
    }

    async fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, Error> {
        let resp = self
            .http
            .post(format!("{AUTH_URL}/token"))
            .form(&self.auth_form(vec![
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ]))
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), body));
        }
        Ok(serde_json::from_str(&body)?)
    }

    // True when no valid token exists in the keyring or the stored token is
    // expired. The server calls login() at startup in that case.
    pub fn needs_login(&self) -> bool {
        match self.load_tokens() {
            Ok(Some(t)) => t.expired(unix_now()),
            _ => true,
        }
    }

    // Returns a valid access token, refreshing and persisting when needed.
    async fn access_token(&self) -> Result<String, Error> {
        let mut guard = self.tokens.lock().await;
        if let Some(t) = guard.as_ref() {
            if !t.expired(unix_now()) {
                return Ok(t.access_token.clone());
            }
        }
        let Some(tokens) = self.load_tokens()? else {
            return Err(Error::NotLoggedIn);
        };
        let auth = self.refresh(&tokens.refresh_token).await?;
        let updated = Tokens {
            access_token: auth.access_token,
            refresh_token: auth.refresh_token.unwrap_or(tokens.refresh_token),
            expires_at: unix_now() + auth.expires_in,
            user_id: tokens.user_id,
            country_code: tokens.country_code,
        };
        self.store_tokens(&updated)?;
        *guard = Some(updated);
        Ok(guard.as_ref().unwrap().access_token.clone())
    }

    // Cached authenticated GET. Errors and non-2xx are never cached.
    // countryCode is required by most Tidal v1 endpoints, so it goes on every
    // request. The cache key includes it, keeping the lookup consistent.
    async fn get_json(&self, path: &str, cache: &Cache<String, Value>) -> Result<Value, Error> {
        let token = self.access_token().await?;
        let mut full = path.to_string();
        if let Some(cc) = self.country_code().await? {
            full.push_str(if full.contains('?') { "&" } else { "?" });
            full.push_str(&format!("countryCode={cc}"));
        }
        if let Some(v) = cache.get(&full).await {
            return Ok(v);
        }
        let resp = self
            .http
            .get(format!("{API_URL}{full}"))
            .bearer_auth(token)
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await?;
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), body.to_string()));
        }
        cache.insert(full, body.clone());
        Ok(body)
    }

    pub async fn album(&self, id: u64) -> Result<Value, Error> {
        self.get_json(&format!("/albums/{id}"), &self.meta_cache).await
    }

    pub async fn artist(&self, id: u64) -> Result<Value, Error> {
        self.get_json(&format!("/artists/{id}"), &self.meta_cache).await
    }

    pub async fn search(&self, query: &str) -> Result<Value, Error> {
        let encoded = percent_encode(query);
        // limit caps each section's page.
        let path = format!("/search?query={encoded}&limit=50");
        self.get_json(&path, &self.search_cache).await
    }

    // Tidal user profile: { username, profileName, firstName, ... }
    pub async fn user_profile(&self) -> Result<Value, Error> {
        let token = self.access_token().await?;
        let user_id = match self.user_id_from_tokens() {
            Some(id) => id,
            None => self.session_with(&token).await?.user_id,
        };
        self.get_json(&format!("/users/{user_id}"), &self.meta_cache)
            .await
    }

    fn user_id_from_tokens(&self) -> Option<u64> {
        self.load_tokens().ok().flatten().and_then(|t| t.user_id)
    }

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

    // Country code from stored tokens, else fetched from the session.
    async fn country_code(&self) -> Result<Option<String>, Error> {
        let cc = self
            .tokens
            .lock()
            .await
            .as_ref()
            .and_then(|t| t.country_code.clone());
        if cc.is_some() {
            return Ok(cc);
        }
        let token = self.access_token().await?;
        let session = self.session_with(&token).await?;
        let mut guard = self.tokens.lock().await;
        if let Some(t) = guard.as_mut() {
            t.country_code = session.country_code.clone();
            t.user_id = Some(session.user_id);
            let _ = self.store_tokens(t);
        }
        Ok(session.country_code)
    }
}

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

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
