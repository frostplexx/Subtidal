use config::Config;
use serde::Deserialize;
use std::path::PathBuf;

// Typed view of the settings file merged with APP_* env vars.
// Add fields here and to settings.toml as the app grows.
#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub username: String,
    pub password: String,
    pub port: u16,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
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
    // Serve word-synced lyrics (enhanced v2 with cueLine/cue timings)
    // from the third-party radiant service. When false, fall back to
    // Tidal's built-in lyrics. NOTE: radiant sends the song metadata
    // (title, artist, isrc, duration, album) to a third-party API.
    #[serde(default = "default_word_synced_lyrics")]
    pub word_synced_lyrics: bool,
    // Exponential backoff on failed logins (2s, 4s, 8s, ... per client
    // IP). Off by default: behind a reverse proxy every client shares
    // the proxy IP, so one attacker can lock out everyone.
    #[serde(default)]
    pub rate_limit: bool,
    // Optional scrobble backends. Both are best-effort: a failing
    // reporter only logs, never fails the client request. The Last.fm
    // session key comes from the OS keychain (run --lastfm-auth once);
    // the ListenBrainz token is the plain API token.
    #[serde(default)]
    pub lastfm: Option<LastFmConfig>,
    #[serde(default)]
    pub listenbrainz: Option<ListenBrainzConfig>,
    // Content labels sent to clients (see LabelsConfig below). Both
    // default to on; set a value to false to stop sending that label.
    #[serde(default)]
    pub labels: LabelsConfig,
}

// Last.fm scrobble credentials. The session key (sk) lives in the OS
// keychain, not here.
#[derive(Debug, Clone, Deserialize)]
pub struct LastFmConfig {
    pub api_key: String,
    pub api_secret: String,
}

// ListenBrainz scrobble credentials: the plain API token.
#[derive(Debug, Clone, Deserialize)]
pub struct ListenBrainzConfig {
    pub token: String,
}

// Content-label toggles ([labels] section in the settings file).
// - ai:       the marker appended to titles of Tidal AI-generated songs
// - explicit: the OpenSubsonic explicitStatus sent for songs and albums
// Both default to on, preserving current behavior when the section is
// missing or a key is absent.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LabelsConfig {
    pub ai: bool,
    pub explicit: bool,
}

impl Default for LabelsConfig {
    fn default() -> Self {
        LabelsConfig {
            ai: true,
            explicit: true,
        }
    }
}

fn default_tidal_quality() -> String {
    "LOSSLESS".into()
}

fn default_show_mixes() -> bool {
    true
}

fn default_word_synced_lyrics() -> bool {
    true
}

fn default_bind_addr() -> String {
    "0.0.0.0".into()
}

// The settings file, in order of preference:
//   1. --config <path> (or -c <path>); the path must exist,
//   2. $XDG_CONFIG_HOME/subtidal/settings.toml,
//   3. ~/.config/subtidal/settings.toml,
//   4. ./settings.toml, the in-repo example (development only).
fn find_config_path() -> Option<PathBuf> {
    if let Some(p) = config_arg() {
        if !p.exists() {
            panic!("config file {} does not exist", p.display());
        }
        return Some(p);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        let p = xdg.join("subtidal").join("settings.toml");
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let p = home.join(".config").join("subtidal").join("settings.toml");
        if p.is_file() {
            return Some(p);
        }
    }
    let local = PathBuf::from("settings.toml");
    local.is_file().then_some(local)
}

// --config <path>, --config=<path>, or -c <path>.
fn config_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if let Some(p) = a.strip_prefix("--config=") {
            return Some(PathBuf::from(p));
        }
        if a == "--config" || a == "-c" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

// Builds the settings from the discovered file and APP_* env vars.
// Eg.. `APP_PASSWORD=x ./target/debug/subtidal` would override the password.
pub fn load_settings() -> Settings {
    let mut builder = Config::builder();
    match find_config_path() {
        Some(p) => {
            builder = builder.add_source(config::File::from(p));
        }
        None => {
            eprintln!("warning: no settings file found; using APP_* env vars only");
            eprintln!(
                "copy settings.toml to $XDG_CONFIG_HOME/subtidal/settings.toml, \
                 or pass --config <path>"
            );
        }
    }
    builder
        .add_source(config::Environment::with_prefix("APP"))
        .build()
        .expect("failed to build settings")
        .try_deserialize()
        .expect("failed to deserialize settings: username, password and port are required")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Settings {
        config::Config::builder()
            .add_source(config::File::from_str(src, config::FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    #[test]
    fn labels_default_to_on_without_section() {
        let s = parse("username = \"u\"\npassword = \"p\"\nport = 8000");
        assert!(s.labels.ai);
        assert!(s.labels.explicit);
    }

    #[test]
    fn labels_can_turn_off_each_independently() {
        let s = parse(
            "username = \"u\"\npassword = \"p\"\nport = 8000\n\
             [labels]\nai = false",
        );
        assert!(!s.labels.ai);
        assert!(s.labels.explicit);

        let s = parse(
            "username = \"u\"\npassword = \"p\"\nport = 8000\n\
             [labels]\nexplicit = false",
        );
        assert!(s.labels.ai);
        assert!(!s.labels.explicit);

        let s = parse(
            "username = \"u\"\npassword = \"p\"\nport = 8000\n\
             [labels]\nai = false\nexplicit = false",
        );
        assert!(!s.labels.ai);
        assert!(!s.labels.explicit);
    }
}
