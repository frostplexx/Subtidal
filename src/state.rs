// Shared credential store: the Tidal token set and the Last.fm session
// key live in one JSON file. One file keeps container persistence simple:
// the Docker image maps SUBTIDAL_TOKEN_FILE to a volume-backed path, and
// both services read and write that same file. Writes create missing
// parent directories and chmod the file to 0600 (owner only).
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

// Section keys in the file.
pub const TIDAL: &str = "tidal";
pub const LASTFM: &str = "lastfm";

// Path override, kept from the keyring era for the Docker image
// (SUBTIDAL_TOKEN_FILE=/data/tokens.json). Without it the store lives at
// $XDG_STATE_HOME/subtidal/state.json, or ~/.local/state/subtidal/state.json
// when XDG_STATE_HOME is unset.
const FILE_ENV: &str = "SUBTIDAL_TOKEN_FILE";

pub fn file_path() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os(FILE_ENV) {
        return Ok(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")));
    base.map(|b| b.join("subtidal").join("state.json"))
        .ok_or_else(|| "no HOME or XDG_STATE_HOME for the credential file".into())
}

// One writer at a time: file IO has no atomic read-modify-write, and
// callers run on many tasks (requests, background re-authorization).
fn file_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn read_doc(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => {
            return Err(format!(
                "credential file read failed ({}): {e}",
                path.display()
            ));
        }
    };
    let doc: Value = serde_json::from_str(&text)
        .map_err(|e| format!("credential file parse failed ({}): {e}", path.display()))?;
    match doc {
        Value::Object(map) => Ok(map),
        _ => Err(format!(
            "credential file ({}): expected a JSON object at the top level",
            path.display()
        )),
    }
}

fn write_doc(path: &Path, doc: &Map<String, Value>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("credential dir create failed ({}): {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(doc)
        .map_err(|e| format!("credential file serialize failed: {e}"))?;
    std::fs::write(path, text)
        .and_then(|_| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        })
        .map_err(|e| format!("credential file write failed ({}): {e}", path.display()))
}

fn load_section_at<T: DeserializeOwned>(path: &Path, key: &str) -> Result<Option<T>, String> {
    let _g = file_lock();
    let doc = read_doc(path)?;
    match doc.get(key) {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| format!("credential section \"{key}\" parse failed: {e}")),
    }
}

fn store_section_at<T: Serialize + ?Sized>(path: &Path, key: &str, value: &T) -> Result<(), String> {
    let _g = file_lock();
    let mut doc = read_doc(path)?;
    let encoded = serde_json::to_value(value)
        .map_err(|e| format!("credential section \"{key}\" serialize failed: {e}"))?;
    doc.insert(key.to_string(), encoded);
    write_doc(path, &doc)
}

// Load one section. None when the section is absent.
pub fn load_section<T: DeserializeOwned>(key: &str) -> Result<Option<T>, String> {
    load_section_at(&file_path()?, key)
}

// Store one section, preserving the others.
pub fn store_section<T: Serialize + ?Sized>(key: &str, value: &T) -> Result<(), String> {
    store_section_at(&file_path()?, key, value)
}

// The whole document as a map. Callers use it to migrate files that
// predate the section layout (legacy Tidal tokens at the root).
pub fn raw_doc() -> Result<Map<String, Value>, String> {
    let _g = file_lock();
    read_doc(&file_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("subtidal-state-test-{}", std::process::id()))
    }

    #[test]
    fn missing_file_reads_as_none() {
        let dir = temp_dir().join("missing");
        let path = dir.join("state.json");
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(load_section_at::<String>(&path, TIDAL).unwrap(), None);
    }

    #[test]
    fn sections_roundtrip_and_do_not_clobber_each_other() {
        let dir = temp_dir().join("roundtrip");
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("state.json");

        store_section_at(&path, TIDAL, &"tid-abc").unwrap();
        assert_eq!(
            load_section_at::<String>(&path, TIDAL).unwrap().unwrap(),
            "tid-abc"
        );
        assert_eq!(load_section_at::<String>(&path, LASTFM).unwrap(), None);

        store_section_at(&path, LASTFM, &"sk123").unwrap();
        assert_eq!(
            load_section_at::<String>(&path, LASTFM).unwrap().unwrap(),
            "sk123"
        );
        // The earlier section survived the read-modify-write.
        assert_eq!(
            load_section_at::<String>(&path, TIDAL).unwrap().unwrap(),
            "tid-abc"
        );
    }

    #[test]
    fn write_creates_parent_dirs_and_sets_0600() {
        let dir = temp_dir().join("perms").join("nested");
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("state.json");

        store_section_at(&path, LASTFM, &"sk").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn corrupt_file_reports_an_error() {
        let dir = temp_dir().join("corrupt");
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("state.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(load_section_at::<String>(&path, TIDAL).is_err());
    }

    #[test]
    fn non_object_file_reports_an_error() {
        let dir = temp_dir().join("not-object");
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("state.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[1, 2]").unwrap();
        assert!(load_section_at::<String>(&path, TIDAL).is_err());
    }
}
