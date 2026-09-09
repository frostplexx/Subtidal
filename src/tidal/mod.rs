pub mod client;
pub mod embedded;
pub mod mapping;
pub mod quality;

use std::sync::OnceLock;

use client::TidalClient;
pub use quality::Quality;

static CLIENT: OnceLock<TidalClient> = OnceLock::new();

pub fn init(client: TidalClient) {
    if CLIENT.set(client).is_err() {
        panic!("tidal client already initialized");
    }
}

pub fn client() -> &'static TidalClient {
    CLIENT.get().expect("tidal client not initialized")
}

// The client when initialized; None in tests and before login.
pub fn client_opt() -> Option<&'static TidalClient> {
    CLIENT.get()
}

// Whether a Tidal session is live. Set at startup when a stored token
// restores, and by the /setup page when a browser login completes. The
// request path reads it on every call, so it is a flag rather than a
// credential-file read; the file is the source of truth, this only
// mirrors it.
static SESSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn mark_logged_in() {
    SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn logged_in() -> bool {
    SESSION.load(std::sync::atomic::Ordering::Relaxed)
}
