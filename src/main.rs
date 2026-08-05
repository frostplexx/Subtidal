mod navidrome;
mod settings;
mod tidal;

use std::sync::OnceLock;

use navidrome::routes::routes;
use settings::{load_settings, Settings};
use tidal::client::TidalClient;
use tracing_subscriber::EnvFilter;

static SETTINGS: OnceLock<Settings> = OnceLock::new();

#[tokio::main]
async fn main() {
    let settings = load_settings();
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
