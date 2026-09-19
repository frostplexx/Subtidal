// Map Tidal artist JSON to Subsonic ArtistID3.
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::{ArtistId3, StarredArtist};
use crate::navidrome::sorting::sort_name;

use super::{album_count_cache, artist_pic_url, cover_cache};

// Size of the artistImageUrl portrait; the same one getStarred2 served
// before the field moved onto ArtistId3.
const IMAGE_SIZE: u32 = 480;

pub fn artist_from_tidal(v: &Value) -> Option<ArtistId3> {
    let id = v["id"].as_u64()?;
    let name = v["name"].as_str()?.to_string();
    let artist_id = ids::encode_artist(id);
    if let Some(pic) = v["picture"].as_str() {
        cover_cache::remember(&artist_id, pic);
    }
    Some(ArtistId3 {
        id: artist_id.clone(),
        // An opaque artist id, not a raw Tidal CDN URL; getCoverArt
        // resolves it back to the artist's picture at request time
        // (cheaply, via the cover_cache populated above).
        cover_art: v["picture"].as_str().map(|_| artist_id),
        artist_image_url: v["picture"].as_str().map(|p| artist_pic_url(p, IMAGE_SIZE)),
        // Tidal reports no count on the artist; the cache holds the size
        // of the release list once it has been fetched (getArtist, the
        // artist directory, or the index's background fill).
        album_count: v["albumCount"]
            .as_u64()
            .map(|n| n as u32)
            .or_else(|| album_count_cache::lookup(id)),
        sort_name: sort_name(&name),
        roles: roles_from_tidal(v),
        name,
        starred: None,
        starred_at: None,
    })
}

// OpenSubsonic roles from Tidal's v1 `artistRoles` categories. Tidal
// names credit categories ("Songwriter", "Producer", ...); the spec
// uses the Picard/MusicBrainz role vocabulary, lowercased. Categories
// with no counterpart pass through lowercased so nothing is lost. v2
// artist shapes (getArtist, search) carry no roles; the list is then
// empty.
pub(crate) fn roles_from_tidal(v: &Value) -> Vec<String> {
    let Some(list) = v["artistRoles"].as_array() else {
        return Vec::new();
    };
    let mut roles: Vec<String> = Vec::new();
    for category in list.iter().filter_map(|r| r["category"].as_str()) {
        let role = match category {
            "Artist" => "artist".to_string(),
            "Songwriter" | "Composer" => "composer".to_string(),
            "Producer" => "producer".to_string(),
            "Engineer" => "engineer".to_string(),
            "Performer" => "performer".to_string(),
            "Mixer" => "mixer".to_string(),
            "Remixer" => "remixer".to_string(),
            "Conductor" => "conductor".to_string(),
            "Arranger" => "arranger".to_string(),
            "Lyricist" => "lyricist".to_string(),
            "DJ" => "djmixer".to_string(),
            other => other.to_lowercase(),
        };
        if !roles.contains(&role) {
            roles.push(role);
        }
    }
    roles
}

// getStarred's legacy artist: name and picture only, plus the favorite time.
// Favorites wrap each artist in { item, created }.
pub fn favorite_artist_from_tidal(entry: &Value) -> Option<StarredArtist> {
    let item = &entry["item"];
    let id = item["id"].as_u64()?;
    let name = item["name"].as_str()?.to_string();
    let artist_id = ids::encode_artist(id);
    if let Some(pic) = item["picture"].as_str() {
        cover_cache::remember(&artist_id, pic);
    }
    Some(StarredArtist {
        id: artist_id.clone(),
        name,
        cover_art: item["picture"].as_str().map(|_| artist_id),
        starred: entry["created"].as_str().map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn artist_maps_fields() {
        let artist = json!({
            "id": 9,
            "name": "The Artist A",
            "picture": "aa-bb-cc",
            "artistRoles": [
                {"category": "Artist", "categoryId": -1},
                {"category": "Songwriter", "categoryId": 2},
                {"category": "Composer", "categoryId": 3},
                {"category": "Sound Designer", "categoryId": 99}
            ]
        });
        let a = artist_from_tidal(&artist).unwrap();
        assert_eq!(a.id, "ar9");
        assert_eq!(a.album_count, None);
        assert_eq!(a.cover_art.as_deref(), Some("ar9"));
        assert_eq!(
            a.artist_image_url.as_deref(),
            Some("https://resources.tidal.com/images/aa/bb/cc/480x480.jpg")
        );
        assert_eq!(a.sort_name, "Artist A, The");
        assert_eq!(a.roles, vec!["artist", "composer", "sound designer"]);
    }

    #[test]
    fn artist_without_roles_or_picture_sends_defaults() {
        let a = artist_from_tidal(&json!({"id": 9, "name": "X"})).unwrap();
        assert!(a.roles.is_empty());
        assert_eq!(a.artist_image_url, None);
        assert_eq!(a.cover_art, None);
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["roles"], json!([]));
        assert_eq!(json["sortName"], "X");
    }

    #[test]
    fn artist_album_count_comes_from_the_release_cache() {
        album_count_cache::remember(4242, 7);
        let a = artist_from_tidal(&json!({"id": 4242, "name": "X"})).unwrap();
        assert_eq!(a.album_count, Some(7));
    }
}
