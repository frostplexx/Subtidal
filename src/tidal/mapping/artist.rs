// Map Tidal artist JSON to Subsonic ArtistID3.
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::ArtistId3;

use super::artist_pic_url;

pub fn artist_from_tidal(v: &Value) -> Option<ArtistId3> {
    let id = v["id"].as_u64()?;
    let name = v["name"].as_str()?.to_string();
    Some(ArtistId3 {
        id: ids::encode_artist(id),
        name,
        cover_art: v["picture"].as_str().map(|p| artist_pic_url(p, 480)),
        album_count: v["albumCount"].as_u64().map(|n| n as u32),
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
        assert_eq!(
            a.cover_art.unwrap(),
            "https://resources.tidal.com/images/pic/1/480x480.jpg"
        );
    }
}
