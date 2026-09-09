// Browser setup wizard at /setup: the headless counterpart to the
// terminal flows. The routes live in routes::public(), so they carry
// their own HTTP Basic check.
use base64::Engine;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use warp::http::{StatusCode, header};
use warp::{Filter, Reply};

use crate::SETTINGS;
use crate::navidrome::scrobble;
use crate::settings::LastFmConfig;
use crate::tidal;

use super::auth::md5_hex;
use super::log::named;

// The steps, in the order the wizard walks them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Tidal,
    LastFm,
}

impl Step {
    fn from_slug(s: &str) -> Option<Self> {
        match s {
            "tidal" => Some(Step::Tidal),
            "lastfm" => Some(Step::LastFm),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Step::Tidal => "Log into Tidal",
            Step::LastFm => "Authorize Last.fm",
        }
    }
}

// The steps this install needs, in order. ListenBrainz needs no
// authorization, so it is not a step.
fn all_steps() -> Vec<Step> {
    let mut steps = vec![Step::Tidal];
    if lastfm_config().is_some() {
        steps.push(Step::LastFm);
    }
    steps
}

fn lastfm_config() -> Option<&'static LastFmConfig> {
    SETTINGS.get()?.lastfm.as_ref()
}

fn done(step: Step) -> bool {
    match step {
        Step::Tidal => tidal::logged_in(),
        Step::LastFm => scrobble::lastfm_session_key().ok().flatten().is_some(),
    }
}

// The first outstanding step, or None when setup is complete.
fn current_step() -> Option<Step> {
    all_steps().into_iter().find(|s| !done(*s))
}

// The Last.fm request token. getSession takes it back, so it must
// outlive the request that created it.
fn pending_lastfm() -> &'static Mutex<Option<String>> {
    static PENDING: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

// Set once the completion page has been shown; later access is 410.
fn completion_shown() -> &'static AtomicBool {
    static SHOWN: std::sync::OnceLock<AtomicBool> = std::sync::OnceLock::new();
    SHOWN.get_or_init(|| AtomicBool::new(false))
}

pub fn setup_routes()
-> impl Filter<Extract = (warp::reply::Response,), Error = warp::Rejection> + Clone {
    let get = warp::path("setup")
        .and(warp::path::end())
        .and(warp::get())
        .and(warp::header::optional::<String>("authorization"))
        .and_then(|auth: Option<String>| async move { gated(auth, None).await });
    let post = warp::path("setup")
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::header::optional::<String>("authorization"))
        .and(warp::body::content_length_limit(64 * 1024))
        .and(warp::body::form::<Vec<(String, String)>>())
        .and_then(
            |auth: Option<String>, form: Vec<(String, String)>| async move {
                gated(auth, Some(form)).await
            },
        );
    get.or(post)
        .unify()
        .map(named("setup"))
        .map(|r: warp::reply::WithHeader<warp::reply::Response>| r.into_response())
}

// Close the wizard once every step is done, then require Basic
// credentials before touching either service.
async fn gated(
    authorization: Option<String>,
    form: Option<Vec<(String, String)>>,
) -> Result<warp::reply::Response, warp::Rejection> {
    // A completed setup is not a permanent surface; returning the reply
    // directly (rather than a rejection) keeps the catch-all from
    // answering it instead.
    let step = match current_step() {
        Some(step) => step,
        None => return Ok(finish_or_gone()),
    };
    if !basic_ok(authorization.as_deref()) {
        return Ok(unauthorized());
    }
    Ok(match form {
        Some(form) => submit(step, &form).await,
        None => start(step).await,
    })
}

// GET: render whichever step is outstanding.
async fn start(step: Step) -> warp::reply::Response {
    match step {
        Step::Tidal => match tidal::client_opt() {
            Some(client) => match client.begin_login().await {
                Ok(url) => step_page(step, StatusCode::OK, &tidal_body(&url), None),
                Err(e) => step_error(step, &e.to_string()),
            },
            None => step_error(step, "The Tidal client is not initialized."),
        },
        Step::LastFm => {
            let Some(cfg) = lastfm_config() else {
                return step_error(step, "Last.fm is not configured.");
            };
            match scrobble::lastfm_begin(&cfg.api_key, &cfg.api_secret).await {
                Ok((token, url)) => {
                    *pending_lastfm().lock().await = Some(token);
                    step_page(step, StatusCode::OK, &lastfm_body(&url), None)
                }
                Err(e) => step_error(step, &e),
            }
        }
    }
}

// POST: finish the step the form was rendered for. A form left open in
// another tab posts a stale step; re-render the current one instead.
async fn submit(step: Step, form: &[(String, String)]) -> warp::reply::Response {
    let posted = field(form, "step").and_then(|s| Step::from_slug(&s));
    if posted != Some(step) {
        return start(step).await;
    }
    match step {
        Step::Tidal => submit_tidal(form).await,
        Step::LastFm => submit_lastfm().await,
    }
}

// Redeem the pasted redirect URL. A failure starts a fresh login, since
// the verifier is consumed either way.
async fn submit_tidal(form: &[(String, String)]) -> warp::reply::Response {
    let pasted = field(form, "url").unwrap_or_default();
    let Some(client) = tidal::client_opt() else {
        return step_error(Step::Tidal, "The Tidal client is not initialized.");
    };
    match client.complete_login(pasted.trim()).await {
        Ok(tokens) => {
            tracing::info!(
                "tidal login succeeded: user_id={} country={}",
                tokens.user_id.unwrap_or(0),
                tokens.country_code.as_deref().unwrap_or("N/A")
            );
            tidal::mark_logged_in();
            advance().await
        }
        Err(e) => {
            let err = e.to_string();
            match client.begin_login().await {
                Ok(url) => step_page(
                    Step::Tidal,
                    StatusCode::BAD_REQUEST,
                    &tidal_body(&url),
                    Some(&err),
                ),
                Err(e2) => step_error(Step::Tidal, &format!("{err} / {e2}")),
            }
        }
    }
}

// Exchange the token handed out by the GET for a session key. The token
// stays valid until used, so a not-yet-authorized answer retries it.
async fn submit_lastfm() -> warp::reply::Response {
    let Some(cfg) = lastfm_config() else {
        return step_error(Step::LastFm, "Last.fm is not configured.");
    };
    let token = pending_lastfm().lock().await.clone();
    let Some(token) = token else {
        return start(Step::LastFm).await;
    };
    match scrobble::lastfm_complete(&cfg.api_key, &cfg.api_secret, &token).await {
        Ok(_) => {
            *pending_lastfm().lock().await = None;
            advance().await
        }
        Err(e) => {
            let url = format!("{LASTFM_AUTH_URL}?api_key={}&token={token}", cfg.api_key);
            step_page(
                Step::LastFm,
                StatusCode::BAD_REQUEST,
                &lastfm_body(&url),
                Some(&e),
            )
        }
    }
}

// After a step completes: show the next one, or the finished page.
async fn advance() -> warp::reply::Response {
    match current_step() {
        Some(next) => start(next).await,
        None => finish_or_gone(),
    }
}

fn field(form: &[(String, String)], name: &str) -> Option<String> {
    form.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.to_string())
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const LASTFM_AUTH_URL: &str = "https://www.last.fm/api/auth/";

fn tidal_body(authorize_url: &str) -> String {
    let url = escape(authorize_url);
    format!(
        "<ol>\
         <li><a href=\"{url}\" target=_blank rel=noopener>Open the Tidal login page</a> \
             and sign in.</li>\
         <li>Tidal then redirects to a page that will not load. </li>\
         <li>Copy that page's full address from the address bar and paste it below.</li>\
         </ol>\
         <form method=post>\
         <input type=hidden name=step value=tidal>\
         <input name=url autofocus autocomplete=off spellcheck=false \
                placeholder=\"https://tidal.com/android/login/auth?code=...\">\
         <button type=submit>Finish this step</button>\
         </form>\
         <p class=hint>Link not clickable? Copy it: <code>{url}</code></p>"
    )
}

fn lastfm_body(authorize_url: &str) -> String {
    let url = escape(authorize_url);
    format!(
        "<ol>\
         <li><a href=\"{url}\" target=_blank rel=noopener>Open the Last.fm authorization \
             page</a> and click Yes, allow access.</li>\
         <li>Come back here and confirm below.</li>\
         </ol>\
         <form method=post>\
         <input type=hidden name=step value=lastfm>\
         <button type=submit>I have authorized Last.fm</button>\
         </form>\
         <p class=hint>Link not clickable? Copy it: <code>{url}</code></p>"
    )
}

// One step, with its position in the wizard and an optional error above.
fn step_page(
    step: Step,
    status: StatusCode,
    body: &str,
    error: Option<&str>,
) -> warp::reply::Response {
    let steps = all_steps();
    let position = steps.iter().position(|s| *s == step).unwrap_or(0) + 1;
    let counter = if steps.len() > 1 {
        format!("<p class=hint>Step {position} of {}</p>", steps.len())
    } else {
        String::new()
    };
    let err = error
        .map(|e| format!("<p class=err>{}</p>", escape(e)))
        .unwrap_or_default();
    page(
        status,
        format!("{counter}<h1>{}</h1>{err}{body}", step.title()),
    )
}

fn step_error(step: Step, message: &str) -> warp::reply::Response {
    step_page(step, StatusCode::BAD_GATEWAY, "", Some(message))
}

// One self-contained page; no external assets.
fn page(status: StatusCode, body: String) -> warp::reply::Response {
    let html = format!(
        "<!doctype html><meta charset=utf-8>\
         <meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>Subtidal setup</title>\
         <style>\
         :root{{color-scheme:light dark}}\
         body{{font:16px/1.5 system-ui,sans-serif;max-width:44rem;margin:3rem auto;padding:0 1rem}}\
         h1{{font-size:1.4rem;margin:.2rem 0 1rem}}\
         input[type=text],input:not([type]){{width:100%;padding:.6rem;font:inherit;box-sizing:border-box}}\
         button{{margin-top:.75rem;padding:.6rem 1.2rem;font:inherit}}\
         code{{word-break:break-all}}\
         .err{{padding:.75rem;border-left:3px solid #c33;background:#c3333311}}\
         .hint{{color:#888;font-size:.9rem}}\
         </style>{body}"
    );
    warp::reply::with_status(warp::reply::html(html), status).into_response()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

// Basic against the configured credentials; the password may be its
// plaintext or MD5 hex.
fn basic_ok(header_value: Option<&str>) -> bool {
    let Some(settings) = SETTINGS.get() else {
        return false;
    };
    basic_matches(header_value, &settings.username, &settings.password)
}

fn basic_matches(header_value: Option<&str>, username: &str, password: &str) -> bool {
    let Some(encoded) = header_value.and_then(|v| v.strip_prefix("Basic ")) else {
        return false;
    };
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
        return false;
    };
    let Ok(decoded) = String::from_utf8(raw) else {
        return false;
    };
    // The password may contain ':', the username may not: split once.
    let Some((user, pass)) = decoded.split_once(':') else {
        return false;
    };
    user == username && (pass == password || pass.to_ascii_lowercase() == md5_hex(password))
}

// First access after setup shows a short confirmation; later access is
// a bare 410 Gone.
fn finish_or_gone() -> warp::reply::Response {
    if !completion_shown().swap(true, Ordering::SeqCst) {
        return page(
            StatusCode::OK,
            "<h1>Setup complete</h1><p>Every configured service is authorized. Close this page.</p>"
                .into(),
        );
    }
    warp::reply::with_status(warp::reply::html(""), StatusCode::GONE).into_response()
}

fn unauthorized() -> warp::reply::Response {
    let mut resp = page(
        StatusCode::UNAUTHORIZED,
        "<p class=err>Sign in with the Subtidal username and password from settings.toml.</p>"
            .into(),
    );
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Basic realm=\"Subtidal\", charset=\"UTF-8\""),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(cred: &str) -> String {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(cred)
        )
    }

    #[test]
    fn basic_auth_accepts_the_configured_credentials() {
        assert!(basic_matches(Some(&basic("u:p")), "u", "p"));
        let hashed = md5_hex("p");
        assert!(basic_matches(
            Some(&basic(&format!("u:{hashed}"))),
            "u",
            "p"
        ));
        assert!(basic_matches(
            Some(&basic(&format!("u:{}", hashed.to_uppercase()))),
            "u",
            "p"
        ));
    }

    #[test]
    fn basic_auth_rejects_everything_else() {
        assert!(!basic_matches(Some(&basic("u:wrong")), "u", "p"));
        assert!(!basic_matches(Some(&basic("wrong:p")), "u", "p"));
        assert!(!basic_matches(Some(&basic("nocolon")), "u", "p"));
        assert!(!basic_matches(Some("Basic !!!not-base64"), "u", "p"));
        assert!(!basic_matches(Some("Bearer xyz"), "u", "p"));
        assert!(!basic_matches(None, "u", "p"));
    }

    #[test]
    fn a_colon_in_the_password_is_preserved() {
        assert!(basic_matches(Some(&basic("u:a:b")), "u", "a:b"));
    }

    #[test]
    fn markup_in_an_error_is_escaped() {
        assert_eq!(
            escape("<script>&\"</script>"),
            "&lt;script&gt;&amp;&quot;&lt;/script&gt;"
        );
    }

    #[test]
    fn the_authorize_url_is_escaped_into_the_form() {
        let html = tidal_body("https://x/authorize?a=1&b=2");
        assert!(html.contains("https://x/authorize?a=1&amp;b=2"), "{html}");
        assert!(!html.contains("?a=1&b=2"), "{html}");
        let html = lastfm_body("https://x/auth?api_key=k&token=t");
        assert!(
            html.contains("https://x/auth?api_key=k&amp;token=t"),
            "{html}"
        );
    }

    #[test]
    fn every_step_form_carries_its_own_slug() {
        assert!(tidal_body("https://x").contains("name=step value=tidal"));
        assert!(lastfm_body("https://x").contains("name=step value=lastfm"));
        assert_eq!(Step::from_slug("tidal"), Some(Step::Tidal));
        assert_eq!(Step::from_slug("lastfm"), Some(Step::LastFm));
        assert_eq!(Step::from_slug("nope"), None);
    }

    // SETTINGS is unset in tests, so Last.fm is unconfigured and Tidal is
    // the only step.
    #[test]
    fn only_configured_services_become_steps() {
        assert_eq!(all_steps(), vec![Step::Tidal]);
    }

    // A logged-out server must not 404 the page it tells people to open.
    #[tokio::test]
    async fn get_setup_asks_for_credentials_when_unconfigured() {
        let resp = warp::test::request()
            .path("/setup")
            .reply(&setup_routes())
            .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
    }

    #[test]
    fn completed_setup_shows_close_page_once_then_answers_gone() {
        completion_shown().store(false, Ordering::SeqCst);
        let first = finish_or_gone();
        assert_eq!(first.status(), StatusCode::OK);
        assert!(
            first
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("text/html"))
        );
        let second = finish_or_gone();
        assert_eq!(second.status(), StatusCode::GONE);
        assert_eq!(second.headers().get(header::CONTENT_LENGTH), None);
        completion_shown().store(false, Ordering::SeqCst);
    }
}
