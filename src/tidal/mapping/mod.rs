// Map Tidal v1 JSON responses to Subsonic models.
// Tidal returns camelCase fields (confirmed against sone and live testing).
// One module per entity; shared helpers stay here, visible to submodules.

pub mod album;
pub mod artist;
pub mod playlist;
pub mod song;

pub use album::{album_from_tidal, favorite_album_from_tidal};
pub use artist::{artist_from_tidal, favorite_artist_from_tidal};
pub use playlist::{mix_from_tidal, mixes_from_page, playlist_from_tidal, playlist_song_from_item};
pub use song::song_from_track;

use serde_json::Value;

// Content-label toggles ([labels] settings section). Both labels default
// to on; tests run without SETTINGS and fall back to those defaults.
pub(crate) fn content_labels() -> crate::settings::LabelsConfig {
    crate::SETTINGS
        .get()
        .map(|s| s.labels.clone())
        .unwrap_or_default()
}

// The OpenSubsonic explicitStatus value for a boolean Tidal flag:
// "explicit" / "clean", omitted entirely when the [labels] setting is off.
pub(crate) fn explicit_status(v: &Value, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }
    v["explicit"]
        .as_bool()
        .map(|e| if e { "explicit" } else { "clean" }.to_string())
}

// Tidal-owned image URLs pass through unchanged. Anything else that
// starts with http is not trusted: treating it as a UUID keeps client
// input from turning the 302 into an open redirect.
pub(crate) fn tidal_image_url(uuid: &str) -> bool {
    let Some(rest) = uuid
        .strip_prefix("https://")
        .or_else(|| uuid.strip_prefix("http://"))
    else {
        return false;
    };
    let Some(host) = rest.split('/').next() else {
        return false;
    };
    host == "tidal.com" || host.ends_with(".tidal.com")
}

// A Tidal image URL whose path ends in /<WxW>.jpg (the artwork files
// served from the images CDN). Rebuilds it with the requested size;
// anything else on a Tidal host passes through unchanged.
fn resize_image_url(url: &str, size: u32) -> Option<String> {
    let prefix = "https://resources.tidal.com/images/";
    let rest = url.strip_prefix(prefix)?;
    let (base, file) = rest.rsplit_once('/')?;
    let file = file.strip_suffix(".jpg")?;
    let (w, h) = file.split_once('x')?;
    if w != h {
        return None;
    }
    Some(format!("{prefix}{base}/{size}x{size}.jpg"))
}

// Square sizes served by the images CDN for album/playlist covers.
// Anything else returns 403, so snap to the nearest size at or above
// the request, and serve the exact size when it exists.
const COVER_SIZES: [u32; 8] = [80, 160, 320, 480, 640, 750, 1080, 1280];
// Artist pictures have a smaller ladder; 640/1280 return 403 there.
const ARTIST_SIZES: [u32; 4] = [160, 320, 480, 750];

fn snap(size: u32, ladder: &[u32]) -> u32 {
    match ladder.binary_search(&size) {
        Ok(_) => size,
        Err(0) => ladder[0],
        Err(i) => ladder[i.min(ladder.len() - 1)],
    }
}

pub fn cover_url(uuid: &str, size: u32) -> String {
    let s = snap(size, &COVER_SIZES);
    if tidal_image_url(uuid) {
        // A full image URL with a baked size (the v2 artwork files) is
        // resized to the requested size, not passed through verbatim.
        return resize_image_url(uuid, s).unwrap_or_else(|| uuid.to_string());
    }
    // Tidal cover UUIDs become slash paths: abc-def -> abc/def.
    let path = uuid.replace('-', "/");
    format!("https://resources.tidal.com/images/{path}/{s}x{s}.jpg")
}

pub fn artist_pic_url(uuid: &str, size: u32) -> String {
    let s = snap(size, &ARTIST_SIZES);
    if tidal_image_url(uuid) {
        return resize_image_url(uuid, s).unwrap_or_else(|| uuid.to_string());
    }
    let path = uuid.replace('-', "/");
    format!("https://resources.tidal.com/images/{path}/{s}x{s}.jpg")
}

pub(crate) fn year_from(s: Option<&str>) -> Option<u32> {
    let s = s?;
    s.get(..4)?.parse().ok()
}

// Every artist credit on an entity: the full `artists` array, or the
// single `artist` object when sparse JSON carries only that. The first
// entry is the lead artist.
pub(crate) fn artist_credits(v: &Value) -> Vec<(u64, String)> {
    if let Some(artists) = v["artists"].as_array() {
        let list: Vec<(u64, String)> = artists
            .iter()
            .filter_map(|a| {
                let id = a["id"].as_u64()?;
                let name = a["name"].as_str()?.to_string();
                Some((id, name))
            })
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    let id = v["artist"]["id"].as_u64();
    let name = v["artist"]["name"].as_str();
    match (id, name) {
        (Some(id), Some(name)) => vec![(id, name.to_string())],
        _ => Vec::new(),
    }
}

// The primary artist on an entity: the entry tagged MAIN when Tidal
// marks one, else the first entry. OpenAPI-flattened artist lists carry
// no type, so those fall back to the first entry. Sparse JSON with only
// the single `artist` object uses that.
pub(crate) fn lead_artist(v: &Value) -> (u64, String) {
    if let Some(artists) = v["artists"].as_array() {
        let pick = artists
            .iter()
            .find(|a| a["type"].as_str() == Some("MAIN"))
            .or_else(|| artists.first());
        if let Some(a) = pick {
            let id = a["id"].as_u64();
            let name = a["name"].as_str();
            if let (Some(id), Some(name)) = (id, name) {
                return (id, name.to_string());
            }
        }
    }
    let id = v["artist"]["id"].as_u64().unwrap_or(0);
    let name = v["artist"]["name"].as_str().unwrap_or("").to_string();
    (id, name)
}

// First page of a Tidal search section: { artists: { items: [...] } }
pub fn search_items<'a>(v: &'a Value, section: &str) -> Vec<&'a Value> {
    v[section]["items"]
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tidal_image_url_whitelists_tidal_hosts_only() {
        assert!(tidal_image_url("https://resources.tidal.com/images/x/640x640.jpg"));
        assert!(tidal_image_url("http://images.tidal.com/pic.jpg"));
        assert!(tidal_image_url("https://tidal.com/x"));
        assert!(!tidal_image_url("https://evil.com/phish"));
        assert!(!tidal_image_url("https://resources.tidal.com.evil.com/x"));
        assert!(!tidal_image_url("https://evil-tidal.com/x"));
        assert!(!tidal_image_url("not-a-url"));
    }

    #[test]
    fn cover_url_passes_through_tidal_urls_but_not_foreign_ones() {
        assert_eq!(
            cover_url("https://resources.tidal.com/images/x/640x640.jpg", 640),
            "https://resources.tidal.com/images/x/640x640.jpg"
        );
        assert_eq!(
            cover_url("https://evil.com/x", 320),
            "https://resources.tidal.com/images/https://evil.com/x/320x320.jpg"
        );
    }

    #[test]
    fn cover_url_resizes_baked_artwork_urls() {
        // A 300px request on a 1280 artwork must serve 320, not 1280.
        assert_eq!(
            cover_url("https://resources.tidal.com/images/abc/def/ghi/1280x1280.jpg", 300),
            "https://resources.tidal.com/images/abc/def/ghi/320x320.jpg"
        );
        // Non-square or unparseable image paths pass through untouched.
        assert_eq!(
            cover_url("https://resources.tidal.com/images/abc/def/18.jpg", 300),
            "https://resources.tidal.com/images/abc/def/18.jpg"
        );
        assert_eq!(
            artist_pic_url("https://resources.tidal.com/images/abc/def/ghi/1280x1280.jpg", 300),
            "https://resources.tidal.com/images/abc/def/ghi/320x320.jpg"
        );
        // An exact CDN size is served verbatim; a size above the ladder
        // clamps to the largest file.
        assert_eq!(
            cover_url("https://resources.tidal.com/images/abc/def/ghi/320x320.jpg", 80),
            "https://resources.tidal.com/images/abc/def/ghi/80x80.jpg"
        );
        assert_eq!(
            cover_url("https://resources.tidal.com/images/abc/def/ghi/640x640.jpg", 2000),
            "https://resources.tidal.com/images/abc/def/ghi/1280x1280.jpg"
        );
        assert_eq!(
            cover_url("https://resources.tidal.com/images/abc/def/ghi/1280x1280.jpg", 480),
            "https://resources.tidal.com/images/abc/def/ghi/480x480.jpg"
        );
    }

    #[test]
    fn search_items_extracts_section() {
        let resp = json!({
            "artists": {"items": [{"id": 1}, {"id": 2}]},
            "albums": {"items": []}
        });
        let artists = search_items(&resp, "artists");
        assert_eq!(artists.len(), 2);
        assert_eq!(search_items(&resp, "albums").len(), 0);
        assert_eq!(search_items(&resp, "missing").len(), 0);
    }
}
