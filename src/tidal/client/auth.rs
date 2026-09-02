// Auth: device-code login, token persistence (keyring or file), refresh,
// and the cached access-token accessor used by every request.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use keyring::{Entry, Error as KeyringError};
use qrcode::{QrCode, render::unicode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Error, AUTH_URL, API_URL, KEYRING_SERVICE, KEYRING_USER, SCOPE};

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
pub(super) struct Tokens {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) expires_at: u64, // unix seconds
    pub(crate) user_id: Option<u64>,
    pub(crate) country_code: Option<String>,
}

impl Tokens {
    fn expired(&self, now: u64) -> bool {
        self.expires_at.saturating_sub(60) <= now
    }
}

impl super::TidalClient {
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
            if v["sub_status"] == 1002 {
                return Err(Error::Auth(
                    "this client_id does not support the device-code flow. \
                     use credentials from the native Android app, not the web player"
                        .into(),
                ));
            }
            if v.get("sub_status").is_some() || v.get("error").is_some() {
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

        println!("Open https://{}/{} in a browser or scan the QR code", auth.verification_uri, auth.user_code);
        let code = QrCode::new(format!("https://{}/{}", auth.verification_uri, auth.user_code)).unwrap();
        let image = code.render::<unicode::Dense1x2>()
            .dark_color(unicode::Dense1x2::Dark)
            .light_color(unicode::Dense1x2::Light)
            .build();

        println!("{}", image);
    

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
    pub(crate) async fn access_token(&self) -> Result<String, Error> {
        let mut guard = self.tokens.lock().await;
        if let Some(t) = guard.as_ref().filter(|t| !t.expired(unix_now())) {
            return Ok(t.access_token.clone());
        }
        let Some(tokens) = self.load_tokens()? else {
            return Err(Error::NotLoggedIn);
        };
        // A stored token that is still valid needs no refresh. This avoids a
        // refresh round trip on every fresh process start.
        if !tokens.expired(unix_now()) {
            let access_token = tokens.access_token.clone();
            *guard = Some(tokens);
            return Ok(access_token);
        }
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

    // Resolve the logged-in user id: stored tokens, else the session.
    pub(crate) async fn user_id(&self) -> Result<u64, Error> {
        let token = self.access_token().await?;
        match self.user_id_from_tokens() {
            Some(id) => Ok(id),
            None => Ok(self.session_with(&token).await?.user_id),
        }
    }

    fn user_id_from_tokens(&self) -> Option<u64> {
        self.load_tokens().ok().flatten().and_then(|t| t.user_id)
    }

    // Country code from stored tokens, else fetched from the session.
    pub(crate) async fn country_code(&self) -> Result<Option<String>, Error> {
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

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
