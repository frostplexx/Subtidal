pub mod client;
pub mod embedded;
pub mod mapping;

use std::sync::OnceLock;

use client::TidalClient;

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
