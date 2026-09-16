// Map Tidal artist JSON to Subsonic ArtistID3.
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::{ArtistId3, StarredArtist};

pub fn artist_from_tidal(v: &Value) -> Option<ArtistId3> {
    let id = v["id"].as_u64()?;
    let name = v["name"].as_str()?.to_string();
    Some(ArtistId3 {
        id: ids::encode_artist(id),
        name,
        // An opaque artist id, not a raw Tidal CDN URL; getCoverArt
        // resolves it back to the artist's picture at request time.
        cover_art: v["picture"].as_str().map(|_| ids::encode_artist(id)),
        album_count: v["albumCount"].as_u64().map(|n| n as u32),
        starred: None,
        starred_at: None,
    })
}

// getStarred's legacy artist: name and picture only, plus the favorite time.
// Favorites wrap each artist in { item, created }.
pub fn favorite_artist_from_tidal(entry: &Value) -> Option<StarredArtist> {
    let item = &entry["item"];
    let id = item["id"].as_u64()?;
    let name = item["name"].as_str()?.to_string();
    Some(StarredArtist {
        id: ids::encode_artist(id),
        name,
        cover_art: item["picture"].as_str().map(|_| ids::encode_artist(id)),
        starred: entry["created"].as_str().map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn artist_maps_fields() {
        let artist = json!({"id": 9, "name": "Artist A", "picture": "pic-1"});
        let a = artist_from_tidal(&artist).unwrap();
        assert_eq!(a.id, "ar9");
        assert_eq!(a.album_count, None);
        assert_eq!(a.cover_art.as_deref(), Some("ar9"));
    }
}
