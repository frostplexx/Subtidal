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

// `subtidal --lastfm-auth`: one-time Last.fm authorization. Prints the
// authorize URL, waits for Enter, then stores the session key in the
// shared credential file and exits.
fn lastfm_auth_flag() -> bool {
    std::env::args().skip(1).any(|a| a == "--lastfm-auth")
}

// `subtidal --version`: print the version and exit. Anything else would
// fall through to load_settings and start a server on the default port.
fn version_flag() -> bool {
    std::env::args().skip(1).any(|a| a == "--version" || a == "-V")
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

    let settings = load_settings();

    print_startup(&settings);
    println!();

    if lastfm_auth_flag() {
        match &settings.lastfm {
            Some(cfg) => {
                match navidrome::scrobble::lastfm_auth_flow(&cfg.api_key, &cfg.api_secret).await {
                    Ok(()) => std::process::exit(0),
                    Err(e) => {
                        eprintln!("lastfm auth failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            None => {
                eprintln!("lastfm auth needs a [lastfm] block in the settings file");
                std::process::exit(1);
            }
        }
    }
    // Automatic first-time authorization: a [lastfm] block without a
    // session key starts the flow on startup. On failure the server
    // still starts without Last.fm scrobbling.
    if let Some(cfg) = &settings.lastfm
        && navidrome::scrobble::lastfm_session_key()
            .ok()
            .flatten()
            .is_none()
    {
        println!("Last.fm is configured but not authorized; starting authorization.");
        if let Err(e) = navidrome::scrobble::lastfm_auth_flow(&cfg.api_key, &cfg.api_secret).await {
            eprintln!("lastfm authorization failed: {e}");
            eprintln!(
                "continuing without Last.fm scrobbling; run `subtidal --lastfm-auth` to retry"
            );
        }
    }
    let client = TidalClient::new(&settings);
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
