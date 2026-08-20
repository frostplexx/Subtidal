mod aligner;
mod navidrome;
mod settings;
mod tidal;

use std::sync::OnceLock;

use navidrome::routes::routes;
use settings::{load_settings, Settings};
use tidal::client::TidalClient;
use tracing_subscriber::EnvFilter;

static SETTINGS: OnceLock<Settings> = OnceLock::new();

// `subtidal --lastfm-auth`: one-time Last.fm authorization. Prints the
// authorize URL, waits for Enter, then stores the session key in the OS
// keychain and exits.
fn lastfm_auth_flag() -> bool {
    std::env::args().skip(1).any(|a| a == "--lastfm-auth")
}

#[tokio::main]
async fn main() {
    let settings = load_settings();
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
        && navidrome::scrobble::lastfm_session_key().ok().flatten().is_none()
    {
        println!("Last.fm is configured but not authorized; starting authorization.");
        if let Err(e) = navidrome::scrobble::lastfm_auth_flow(&cfg.api_key, &cfg.api_secret).await {
            eprintln!("lastfm authorization failed: {e}");
            eprintln!("continuing without Last.fm scrobbling; run `subtidal --lastfm-auth` to retry");
        }
    }
    let client = TidalClient::new(&settings);
    if client.needs_login() {
        println!("Tidal login required:");
        if let Err(e) = client.login().await {
            eprintln!("login failed: {e}");
            std::process::exit(1);
        }
    }
    tidal::init(client);
    SETTINGS.set(settings).expect("SETTINGS already set");
    navidrome::scrobble::init(SETTINGS.get().unwrap());
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let routes = routes();
    println!("Server started at http://localhost:{}", SETTINGS.get().unwrap().port);
    warp::serve(routes).run(([0, 0, 0, 0], SETTINGS.get().unwrap().port)).await;
}
