// Auth: device-code login, token persistence (shared credential file),
// refresh, and the cached access-token accessor used by every request.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use qrcode::{QrCode, render::unicode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{API_URL, AUTH_URL, Error, SCOPE};

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

    // Tokens persist in the shared credential file (src/state.rs) under
    // the "tidal" section. The Docker image points SUBTIDAL_TOKEN_FILE at
    // a volume-backed file; without the override the file defaults to
    // $XDG_STATE_HOME/subtidal/state.json.
    fn store_tokens(&self, tokens: &Tokens) -> Result<(), Error> {
        crate::state::store_section(crate::state::TIDAL, tokens).map_err(Error::Auth)
    }

    fn load_tokens(&self) -> Result<Option<Tokens>, Error> {
        match crate::state::load_section::<Tokens>(crate::state::TIDAL) {
            Ok(Some(t)) => Ok(Some(t)),
            Ok(None) => Self::legacy_root_tokens(),
            Err(e) => Err(Error::Auth(e)),
        }
    }

    // Files written before the unified store kept the Tokens object at
    // the document root. Read those so an upgrade does not force a
    // re-login.
    fn legacy_root_tokens() -> Result<Option<Tokens>, Error> {
        let doc = crate::state::raw_doc().map_err(Error::Auth)?;
        if doc.contains_key("access_token") {
            serde_json::from_value(serde_json::Value::Object(doc))
                .map(Some)
                .map_err(Error::Json)
        } else {
            Ok(None)
        }
    }

    // Device-code login. Prints the code for the CLI, polls until the user
    // authorizes, then persists the tokens to the shared credential file.
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
                return Err(Error::Auth(format!("device authorization refused: {msg}")));
            }
        }
        let auth: DeviceAuth = serde_json::from_str(&body)?;

        println!(
            "Open https://{}/{} in a browser or scan the QR code to log into Tidal.",
            auth.verification_uri, auth.user_code
        );
        let code = QrCode::new(format!(
            "https://{}/{}",
            auth.verification_uri, auth.user_code
        ))
        .unwrap();
        let image = code
            .render::<unicode::Dense1x2>()
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
                "Logged in.\nuser_id={} country={:?}",
                tokens.user_id.unwrap_or(0),
                tokens.country_code.unwrap_or("N/A".to_string())
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

    // Restore a session at startup: use a stored token, refresh an expired
    // one silently, and only fall back to the full device-code login when
    // no token exists or Tidal rejects the refresh. HTTP 400/401 means the
    // stored refresh token was revoked or expired.
    pub async fn ensure_session(&self) -> Result<(), Error> {
        let Some(tokens) = self.load_tokens()? else {
            return self.login().await;
        };
        if !tokens.expired(unix_now()) {
            return Ok(());
        }
        match self.refresh_and_store(&tokens).await {
            Ok(_) => Ok(()),
            Err(Error::Tidal(400 | 401, _)) => {
                println!(
                    "The stored Tidal session expired and could not be refreshed; logging in again."
                );
                self.login().await
            }
            Err(e) => Err(e),
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
        let updated = self.refresh_and_store(&tokens).await?;
        *guard = Some(updated);
        Ok(guard.as_ref().unwrap().access_token.clone())
    }

    // Exchange a stored refresh token for fresh tokens and persist them.
    // The in-memory cache is left untouched, so callers may hold the
    // tokens lock across the await.
    async fn refresh_and_store(&self, tokens: &Tokens) -> Result<Tokens, Error> {
        let auth = self.refresh(&tokens.refresh_token).await?;
        let updated = Tokens {
            access_token: auth.access_token,
            refresh_token: auth
                .refresh_token
                .unwrap_or_else(|| tokens.refresh_token.clone()),
            expires_at: unix_now() + auth.expires_in,
            user_id: tokens.user_id,
            country_code: tokens.country_code.clone(),
        };
        self.store_tokens(&updated)?;
        Ok(updated)
    }

    // Resolve the logged-in user id: stored tokens, else the session.
    pub(crate) async fn user_id(&self) -> Result<u64, Error> {
        let token = self.access_token().await?;
        match self.user_id_from_tokens() {
            Some(id) => Ok(id),
            None => Ok(self.session_with(&token).await?.user_id),
        }
    }

    pub(crate) fn user_id_from_tokens(&self) -> Option<u64> {
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
