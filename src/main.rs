mod flac;
mod navidrome;
mod settings;
mod state;
mod tidal;

use std::sync::OnceLock;

use navidrome::routes::routes;
use settings::{Settings, load_settings};
use tidal::client::TidalClient;
use tracing_subscriber::EnvFilter;

use crate::settings::LabelsConfig;

static SETTINGS: OnceLock<Settings> = OnceLock::new();

// `subtidal --version`: print the version and exit. Anything else would
// fall through to load_settings and start a server on the default port.
fn version_flag() -> bool {
    std::env::args().skip(1).any(|a| a == "--version" || a == "-V")
}

// The subcommand, if one was given. Unrecognized arguments are ignored
// rather than rejected, preserving the previous behaviour for flags the
// server does not know.
fn subcommand() -> Option<String> {
    std::env::args()
        .nth(1)
        .filter(|a| a == "login" || a == "logout")
}

// `subtidal logout`: discard the stored Tidal session and exit. Needed
// because a session is bound to the client that minted it — after
// changing credentials the old refresh token keeps working and silently
// pins you to the old client's entitlements, so re-authorizing has to be
// explicit.
fn logout() -> ! {
    match state::clear_section(state::TIDAL) {
        Ok(()) => {
            println!("Logged out. Run `subtidal login` to authorize again.");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("could not clear the stored session: {e}");
            std::process::exit(1);
        }
    }
}

fn print_startup(s: &Settings) {
    println!("Subtidal v{}", env!("CARGO_PKG_VERSION"));
    println!();
    let rows: Vec<(String, String)> = vec![
        ("username".into(), s.username.clone()),
        ("password".into(), "********".into()),
        ("port".into(), s.port.to_string()),
        ("address".into(), s.bind_addr.to_string()),
        ("tidal quality".into(), s.tidal_quality.clone()),
        ("show mixes".into(), on_off(s.show_mixes)),
        ("word-synced lyrics".into(), on_off(s.word_synced_lyrics)),
        ("rate limit".into(), on_off(s.rate_limit)),
        ("content labels".into(), labels_str(&s.labels)),
        ("lastfm".into(), on_off(s.lastfm.is_some())),
        ("listenbrainz".into(), on_off(s.listenbrainz.is_some())),
    ];
    let w = rows.iter().map(|(k, _)| k.len()).max().unwrap();
    for (k, v) in rows {
        println!("  {k:<w$}  {v}");
    }
}

fn on_off(b: bool) -> String {
    if b { "on".into() } else { "off".into() }
}

fn labels_str(l: &LabelsConfig) -> String {
    format!("ai {}, explicit {}", on_off(l.ai), on_off(l.explicit))
}

#[tokio::main]
async fn main() {
    // --version/-V print and exit before settings load, so the flag
    // cannot start a server on the default port by accident.
    if version_flag() {
        println!("Subtidal v{}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    let cmd = subcommand();
    if cmd.as_deref() == Some("logout") {
        logout();
    }

    let settings = load_settings();

    print_startup(&settings);
    println!();

    // Automatic first-time authorization: a [lastfm] block without a
    // session key starts the flow on startup, which prints the authorize
    // URL and QR code. On failure the server still starts without
    // Last.fm scrobbling.
    if let Some(cfg) = &settings.lastfm
        && navidrome::scrobble::lastfm_session_key()
            .ok()
            .flatten()
            .is_none()
    {
        println!("Last.fm is configured but not authorized; starting authorization.");
        if let Err(e) = navidrome::scrobble::lastfm_auth_flow(&cfg.api_key, &cfg.api_secret).await {
            eprintln!("lastfm authorization failed: {e}");
            eprintln!("continuing without Last.fm scrobbling.");
        }
    }
    let client = TidalClient::new(&settings);
    // `subtidal login` re-authorizes unconditionally and exits. Going
    // through ensure_session instead would just refresh the session that
    // is already stored and do nothing visible.
    if cmd.as_deref() == Some("login") {
        match client.login().await {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("login failed: {e}");
                std::process::exit(1);
            }
        }
    }
    // Restore the stored session silently (refresh-first); only a dead
    // refresh token forces the interactive login.
    if let Err(e) = client.ensure_session().await {
        eprintln!("login failed: {e}");
        std::process::exit(1);
    }
    tidal::init(client);
    SETTINGS.set(settings).expect("SETTINGS already set");
    navidrome::scrobble::init(SETTINGS.get().unwrap());
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    // Report the account's real ceiling once. A stream served below the
    // configured tier is otherwise indistinguishable from a bug, because
    // playbackinfo downgrades silently instead of refusing.
    match tidal::client().subscription().await {
        Ok(sub) => {
            tracing::info!(
                "tidal subscription: {}, highest sound quality {}",
                sub["subscription"]["type"].as_str().unwrap_or("unknown"),
                sub["highestSoundQuality"].as_str().unwrap_or("unknown"),
            );
            // The two fields above can disagree (a legacy
            // highestSoundQuality outliving a plan change), so keep the
            // whole object available rather than only the reading of it.
            tracing::debug!("tidal subscription detail: {sub}");
        }
        Err(e) => tracing::warn!("could not read tidal subscription: {e}"),
    }
    // Which registered client the token belongs to. Sound quality is
    // scoped per client, so this is the ceiling that applies when the
    // subscription itself allows more than the streams come back as.
    match tidal::client().session_raw().await {
        Ok(s) => tracing::info!(
            "tidal client: {} (id {})",
            s["client"]["name"].as_str().unwrap_or("unknown"),
            s["client"]["id"].as_i64().unwrap_or(-1),
        ),
        Err(e) => tracing::warn!("could not read tidal session: {e}"),
    }

    let routes = routes();
    let settings = SETTINGS.get().unwrap();
    let bind = settings
        .bind_addr
        .parse::<std::net::IpAddr>()
        .expect("bind_addr in settings must be an IP address");
    println!("Listening on http://{bind}:{}", settings.port);
    warp::serve(routes)
        .run((bind, SETTINGS.get().unwrap().port))
        .await;
}
