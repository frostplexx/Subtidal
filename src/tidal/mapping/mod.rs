// Map Tidal v1 JSON responses to Subsonic models.
// Tidal returns camelCase fields (confirmed against sone and live testing).
// One module per entity; shared helpers stay here, visible to submodules.
//
// Where an endpoint's pieces live:
//   tidal/client.rs      the HTTP call, returns raw serde_json::Value
//   tidal/mapping/       Value -> Subsonic struct (this directory)
//   navidrome/models.rs  the Subsonic response structs
//   navidrome/handlers.rs composes the three, wraps in the envelope
//   navidrome/routes.rs  one dispatch arm per endpoint name
pub mod album;
pub mod artist;
pub mod playlist;
pub mod song;

pub use album::{album_from_tidal, favorite_album_from_tidal};
pub use artist::{artist_from_tidal, favorite_artist_from_tidal};
pub use playlist::{mix_from_tidal, mixes_from_page, playlist_from_tidal, playlist_song_from_item};
pub use song::song_from_track;

use serde_json::Value;

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

// Album cover sizes: 160/320/640/1280. Snap the requested size upward.
pub fn cover_url(uuid: &str, size: u32) -> String {
    if tidal_image_url(uuid) {
        return uuid.to_string();
    }
    // Tidal cover UUIDs become slash paths: abc-def -> abc/def.
    let path = uuid.replace('-', "/");
    let snapped = match size {
        0..=160 => 160,
        161..=320 => 320,
        321..=640 => 640,
        _ => 1280,
    };
    format!("https://resources.tidal.com/images/{path}/{snapped}x{snapped}.jpg")
}

// Artist pictures only exist at 160/320/480/750; 640 and 1280 403.
pub fn artist_pic_url(uuid: &str, size: u32) -> String {
    if tidal_image_url(uuid) {
        return uuid.to_string();
    }
    let path = uuid.replace('-', "/");
    let snapped = match size {
        0..=160 => 160,
        161..=320 => 320,
        321..=480 => 480,
        _ => 750,
    };
    format!("https://resources.tidal.com/images/{path}/{snapped}x{snapped}.jpg")
}

pub(crate) fn year_from(s: Option<&str>) -> Option<u32> {
    let s = s?;
    s.get(..4)?.parse().ok()
}

// First artist from `artists`, or the single `artist` object.
// Multi-artist names join as "A feat. B, C".
fn primary_artist(v: &Value) -> (u64, String) {
    if let Some(artists) = v["artists"].as_array() {
        let id = artists
            .first()
            .and_then(|a| a["id"].as_u64())
            .unwrap_or(0);
        let names: Vec<&str> = artists.iter().filter_map(|a| a["name"].as_str()).collect();
        let name = match names.split_first() {
            Some((first, rest)) => {
                let mut joined = first.to_string();
                if !rest.is_empty() {
                    joined.push_str(" feat. ");
                    joined.push_str(&rest.join(", "));
                }
                joined
            }
            None => String::new(),
        };
        (id, name)
    } else {
        let id = v["artist"]["id"].as_u64().unwrap_or(0);
        let name = v["artist"]["name"].as_str().unwrap_or("").to_string();
        (id, name)
    }
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
            cover_url("https://resources.tidal.com/images/x/640x640.jpg", 320),
            "https://resources.tidal.com/images/x/640x640.jpg"
        );
        assert_eq!(
            cover_url("https://evil.com/x", 320),
            "https://resources.tidal.com/images/https://evil.com/x/320x320.jpg"
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
