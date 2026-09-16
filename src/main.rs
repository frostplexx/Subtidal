mod flac;
mod navidrome;
mod settings;
mod state;
mod tidal;
mod transcode;

use std::sync::OnceLock;
use std::time::Duration;

use navidrome::routes::routes;
use settings::{Settings, load_settings};
use tidal::client::TidalClient;
use tracing_subscriber::EnvFilter;

use crate::settings::LabelsConfig;

static SETTINGS: OnceLock<Settings> = OnceLock::new();

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

fn logout() -> ! {
    // Clear both credential sections: finishing a logout must not leave
    // Last.fm authorized when Tidal is not.
    match state::clear_section(state::TIDAL).and_then(|()| state::clear_section(state::LASTFM)) {
        Ok(()) => {
            println!("Logged out of Tidal and Last.fm. Run `subtidal login` to authorize again.");
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
        ("transcode".into(), on_off(s.transcode.enabled)),
        ("lastfm".into(), on_off(s.lastfm.is_some())),
        ("listenbrainz".into(), on_off(s.listenbrainz.is_some())),
    ];
    let w = rows.iter().map(|(k, _)| k.len()).max().unwrap();
    for (k, v) in rows {
        println!("  {k:<w$}  {v}");
    }
}

// Transcoding defaults to on, so a host without ffmpeg would answer every
// lossy-format request with a 200 whose body fails immediately. Turning it off
// puts those requests back on the tier mapping, which serves Tidal's own lossy
// asset and actually plays.
async fn disable_transcode_without_ffmpeg(settings: &mut Settings) -> Option<String> {
    if !settings.transcode.enabled {
        return None;
    }
    let bin = settings::ffmpeg_bin(settings);
    if transcode::ffmpeg_available(&bin).await {
        return None;
    }
    settings.transcode.enabled = false;
    Some(bin)
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

    let mut settings = load_settings();
    let missing_ffmpeg = disable_transcode_without_ffmpeg(&mut settings).await;

    print_startup(&settings);
    if let Some(bin) = missing_ffmpeg {
        // Logging is not initialized this early, so this goes to stderr.
        eprintln!();
        eprintln!("  transcoding is off: could not run {bin:?}. Lossy-format");
        eprintln!("  requests fall back to Tidal's own lossy streams.");
    }
    println!();

    let client = TidalClient::new(&settings);

    if cmd.as_deref() == Some("login") {
        match client.login().await {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("login failed: {e}");
                std::process::exit(1);
            }
        }
    }
    // Restore the stored session silently (refresh-first). A missing or
    // revoked token is not fatal: the server still comes up, serving the
    // /setup page and nothing else. That is the only way to authorize a
    // headless install, where there is no stdin to prompt on.
    let logged_in = match client.restore_session().await {
        Ok(()) => {
            tidal::mark_logged_in();
            true
        }
        Err(tidal::client::Error::NotLoggedIn) => false,
        Err(e) => {
            eprintln!("login failed: {e}");
            std::process::exit(1);
        }
    };
    tidal::init(client);
    SETTINGS.set(settings).expect("SETTINGS already set");
    navidrome::scrobble::init(SETTINGS.get().unwrap());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            // warp logs every aborted client connection (e.g. a player
            // cancelling a request mid-navigation) as an ERROR-level
            // IncompleteMessage; that's normal client behavior, not a
            // server fault, so it's muted by default.
            EnvFilter::new("info,warp::server::run=off")
        }))
        .init();

    if logged_in {
        match tidal::client().session_raw().await {
            Ok(s) => tracing::info!(
                "tidal client: {} (id {})",
                s["client"]["name"].as_str().unwrap_or("unknown"),
                s["client"]["id"].as_i64().unwrap_or(-1),
            ),
            Err(e) => tracing::warn!("could not read tidal session: {e}"),
        }
    }

    let routes = routes();
    let settings = SETTINGS.get().unwrap();
    let bind = settings
        .bind_addr
        .parse::<std::net::IpAddr>()
        .expect("bind_addr in settings must be an IP address");
    println!("Listening on http://{bind}:{}", settings.port);
    // A wildcard bind is not a reachable address; name the port and let
    // the operator supply the host.
    let host = if bind.is_unspecified() {
        "<this-host>".to_string()
    } else {
        bind.to_string()
    };
    let setup_url = format!("http://{host}:{}/setup", settings.port);
    // The /setup wizard owns first-time authorization now: Tidal always,
    // Last.fm when a [lastfm] block exists without a session key. Nothing
    // is prompted on stdin here, because headless installs have none.
    let lastfm_pending = settings.lastfm.is_some()
        && navidrome::scrobble::lastfm_session_key()
            .ok()
            .flatten()
            .is_none();
    match (logged_in, lastfm_pending) {
        (false, _) => println!(
            "Not logged into Tidal. Open {setup_url} in a browser and sign in\n\
             (username and password are the ones from settings.toml)."
        ),
        (true, true) => println!(
            "Last.fm is configured but not authorized. Open {setup_url} in a browser\n\
             and complete its step."
        ),
        (true, false) => {}
    }
    serve_with_keepalive(routes, bind, SETTINGS.get().unwrap().port).await;
}

// warp::serve()'s TcpListener never turns on SO_KEEPALIVE for accepted
// connections (tokio does not enable it by default, unlike Go's
// net/http.Server, which wraps every accepted connection in a listener
// that sets a 3-minute keepalive period). Without it, a connection a
// client is holding open and reusing can go idle, get silently dropped
// by a router/AP/OS power-saving path, and neither side notices until
// the client tries to write to it - which surfaces on iOS as a generic
// "Socket is not connected" background/foreground request failure.
// Enabling keepalive here lets the OS notice and clean up (or keep
// alive) a stale connection instead of leaving it a silent trap.
//
// This bypasses warp::serve()'s convenience TcpListener Accept impl,
// which is the only way to touch the accepted socket before hyper takes
// it; warp's own remote-address extension type is crate-private, so the
// remote address is instead carried as a `SocketAddr` request extension
// of our own and read back via `warp::filters::ext::optional` (see
// navidrome::auth and navidrome::log, which read it the same way
// `warp::addr::remote()` normally would).
const TCP_KEEPALIVE: Duration = Duration::from_secs(60);

async fn serve_with_keepalive<F>(routes: F, bind: std::net::IpAddr, port: u16)
where
    F: warp::Filter<Error = warp::Rejection> + Clone + Send + Sync + 'static,
    F::Extract: warp::Reply,
{
    let listener = tokio::net::TcpListener::bind((bind, port))
        .await
        .unwrap_or_else(|e| panic!("failed to bind {bind}:{port}: {e}"));
    loop {
        let (stream, remote) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("failed to accept connection: {e}");
                continue;
            }
        };
        if let Err(e) = socket2::SockRef::from(&stream).set_tcp_keepalive(
            &socket2::TcpKeepalive::new()
                .with_time(TCP_KEEPALIVE)
                .with_interval(TCP_KEEPALIVE),
        ) {
            tracing::debug!("failed to enable tcp keepalive for {remote}: {e}");
        }
        let routes = routes.clone();
        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            let svc = hyper_util::service::TowerToHyperService::new(WithRemoteAddr {
                inner: warp::service(routes),
                remote,
            });
            if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                hyper_util::rt::TokioExecutor::new(),
            )
            .serve_connection_with_upgrades(io, svc)
            .await
            {
                tracing::debug!("connection error: {e}");
            }
        });
    }
}

// Stamps the accepted connection's remote address onto every request
// that flows through it, standing in for warp's own (crate-private)
// remote-address extension since serve_with_keepalive bypasses
// warp::serve()'s built-in listener.
#[derive(Clone)]
struct WithRemoteAddr<S> {
    inner: S,
    remote: std::net::SocketAddr,
}

impl<S, B> tower_service::Service<http::Request<B>> for WithRemoteAddr<S>
where
    S: tower_service::Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        req.extensions_mut().insert(self.remote);
        self.inner.call(req)
    }
}
