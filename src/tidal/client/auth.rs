// Auth: Authorization Code + PKCE login, token persistence (shared
// credential file), refresh, and the cached access-token accessor used by
// every request.
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use qrcode::{QrCode, render::unicode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{API_URL, AUTH_URL, Error, SCOPE};

// Tidal's hosted authorize page, and the redirect it sends the browser to
// on success. The redirect target is a real Tidal URL that does not
// render anything useful — it exists only to carry `?code=` back, which
// is what makes the flow work without a local callback server.
const AUTHORIZE_URL: &str = "https://login.tidal.com/authorize";
const PKCE_REDIRECT_URI: &str = "https://tidal.com/android/login/auth";

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

    pub async fn login(&self) -> Result<(), Error> {
        if self.client_id.starts_with("REPLACE_") {
            return Err(Error::Auth(
                "Tidal credentials are not configured. Run:\n  \
                 python3 scripts/gen_embedded.py CLIENT_ID CLIENT_SECRET > src/tidal/embedded.rs\n\
                 then rebuild, or set tidal_client_id / tidal_client_secret in settings.toml"
                    .into(),
            ));
        }

        let (verifier, challenge) = pkce_pair();
        let unique_key = client_unique_key();
        let query = serde_urlencoded::to_string([
            ("response_type", "code"),
            ("redirect_uri", PKCE_REDIRECT_URI),
            ("client_id", self.client_id.as_str()),
            ("lang", "EN"),
            ("appMode", "android"),
            ("client_unique_key", unique_key.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("restrict_signup", "true"),
        ])
        .map_err(|e| Error::Auth(format!("could not build authorize url: {e}")))?;
        let authorize_url = format!("{AUTHORIZE_URL}?{query}");

        println!("Open this URL in a browser or scan the QR code to log into Tidal:\n");
        println!("{authorize_url}\n");
        if let Ok(code) = QrCode::new(&authorize_url) {
            println!(
                "{}",
                code.render::<unicode::Dense1x2>()
                    .dark_color(unicode::Dense1x2::Dark)
                    .light_color(unicode::Dense1x2::Light)
                    .build()
            );
        }
        println!(
            "After signing in the browser is redirected to a {PKCE_REDIRECT_URI} page\n\
             that will not load. That is expected — the login already succeeded.\n\
             Copy that page's full address from the address bar and paste it here."
        );
        print!("\nRedirect URL: ");
        use std::io::Write;
        std::io::stdout().flush().ok();

        // stdin is blocking; keep it off the async runtime's worker.
        let line = tokio::task::spawn_blocking(|| {
            let mut buf = String::new();
            std::io::stdin().read_line(&mut buf).map(|_| buf)
        })
        .await
        .map_err(|e| Error::Auth(format!("could not read stdin: {e}")))?
        .map_err(|e| Error::Auth(format!("could not read stdin: {e}")))?;

        let code = authorization_code(line.trim()).ok_or_else(|| {
            Error::Auth(
                "no authorization code found. Paste the whole redirect URL \
                 (it contains `?code=...`), or just the code itself."
                    .into(),
            )
        })?;

        let resp = self
            .http
            .post(format!("{AUTH_URL}/token"))
            .form(&self.auth_form(vec![
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("code_verifier", &verifier),
                ("redirect_uri", PKCE_REDIRECT_URI),
                ("client_unique_key", &unique_key),
                ("scope", SCOPE),
            ]))
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            // An already-redeemed or stale code is the common mistake and
            // reads as a generic invalid_grant otherwise.
            if body.contains("invalid_grant") {
                return Err(Error::Auth(
                    "the authorization code was rejected. Codes are single-use and \
                     short-lived — run the login again and paste the new URL promptly."
                        .into(),
                ));
            }
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
        Ok(())
    }

    pub async fn session_raw(&self) -> Result<Value, Error> {
        let token = self.access_token().await?;
        let resp = self
            .http
            .get(format!("{API_URL}/sessions"))
            .bearer_auth(token)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(Error::Tidal(status.as_u16(), body));
        }
        Ok(serde_json::from_str(&body)?)
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

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_pair() -> (String, String) {
    let raw: [u8; 32] = rand::random();
    let verifier = b64url(&raw);
    // The challenge hashes the verifier's ASCII form, not the bytes it
    // decodes to (RFC 7636 §4.2).
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn client_unique_key() -> String {
    let raw: [u8; 8] = rand::random();
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

fn authorization_code(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let Some((_, query)) = input.split_once('?') else {
        // Not a URL: treat it as the code itself, unless it is obviously
        // a URL whose code is missing.
        return (!input.starts_with("http")).then(|| input.to_string());
    };
    // Strip a fragment before parsing; some browsers append one.
    let query = query.split('#').next().unwrap_or(query);
    serde_urlencoded::from_str::<Vec<(String, String)>>(query)
        .ok()?
        .into_iter()
        .find(|(k, v)| k == "code" && !v.is_empty())
        .map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_the_sha256_of_the_verifier_string() {
        let (verifier, challenge) = pkce_pair();
        // Both are base64url without padding, so neither needs escaping
        // in a query string.
        for s in [&verifier, &challenge] {
            assert!(!s.contains('='), "padding would need escaping: {s}");
            assert!(!s.contains('+') && !s.contains('/'), "not url-safe: {s}");
        }
        assert_eq!(challenge, b64url(&Sha256::digest(verifier.as_bytes())));
        // A fresh pair every call; a reused verifier would let one
        // intercepted authorize URL be replayed.
        assert_ne!(pkce_pair().0, verifier);
    }

    #[test]
    fn rfc7636_reference_vector() {
        // RFC 7636 appendix B: the known verifier/challenge pair.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            b64url(&Sha256::digest(verifier.as_bytes())),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn code_is_read_from_a_pasted_redirect_url() {
        assert_eq!(
            authorization_code("https://tidal.com/android/login/auth?code=abc123&state=x"),
            Some("abc123".into())
        );
        // Order does not matter.
        assert_eq!(
            authorization_code("https://tidal.com/android/login/auth?state=x&code=abc123"),
            Some("abc123".into())
        );
        // A trailing fragment must not end up inside the code.
        assert_eq!(
            authorization_code("https://tidal.com/android/login/auth?code=abc123#_=_"),
            Some("abc123".into())
        );
        // Percent-encoding is decoded.
        assert_eq!(
            authorization_code("https://x/cb?code=a%2Bb"),
            Some("a+b".into())
        );
    }

    #[test]
    fn a_bare_code_is_accepted() {
        assert_eq!(authorization_code("abc123"), Some("abc123".into()));
        assert_eq!(authorization_code("  abc123  "), Some("abc123".into()));
    }

    #[test]
    fn junk_and_codeless_urls_are_rejected() {
        assert_eq!(authorization_code(""), None);
        assert_eq!(authorization_code("   "), None);
        // A URL that carries no code must not be mistaken for a code.
        assert_eq!(authorization_code("https://tidal.com/android/login/auth"), None);
        assert_eq!(authorization_code("https://x/cb?error=access_denied"), None);
        // An empty code value is not a code.
        assert_eq!(authorization_code("https://x/cb?code="), None);
    }

    #[test]
    fn unique_key_is_hex_and_fresh_per_login() {
        let k = client_unique_key();
        assert_eq!(k.len(), 16);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(client_unique_key(), k);
    }
}
