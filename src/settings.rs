use config::Config;
use serde::Deserialize;

// Typed view of ./settings.toml merged with APP_* env vars.
// Add fields here and to settings.toml as the app grows.
#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub username: String,
    pub password: String,
    pub port: u16,
    // Optional override for the embedded Tidal app credentials.
    // Leave empty to use the embedded defaults.
    #[serde(default)]
    pub tidal_client_id: Option<String>,
    #[serde(default)]
    pub tidal_client_secret: Option<String>,
    // Default stream quality when a client sends no bitrate or format
    // hint: LOSSLESS | HIGH | LOW. The client's own hints still win.
    #[serde(default = "default_tidal_quality")]
    pub tidal_quality: String,
    // Blend the Tidal mixes (Daily Mix, My Mix, Discovery) into
    // getPlaylists as read-only playlists.
    #[serde(default = "default_show_mixes")]
    pub show_mixes: bool,
}

fn default_tidal_quality() -> String {
    "LOSSLESS".into()
}

fn default_show_mixes() -> bool {
    true
}

// Builds the settings from ./settings.toml and APP_* env vars.
// Eg.. `APP_PASSWORD=x ./target/debug/subtidal` would override the password.
pub fn load_settings() -> Settings {
    Config::builder()
        // Add in `./settings.toml`
        .add_source(config::File::with_name("settings"))
        // Add in settings from the environment (with a prefix of APP)
        .add_source(config::Environment::with_prefix("APP"))
        .build()
        .expect("failed to build settings")
        .try_deserialize()
        .expect("failed to deserialize settings")
}
