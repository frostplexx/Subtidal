// Map Tidal playlist JSON to Subsonic Playlist.
use serde_json::Value;

use crate::navidrome::models::Playlist;

use super::{cover_url, song::song_from_track};

// Tidal playlist ids are UUIDs; Subsonic keeps them as opaque strings.
// squareImage is a cover UUID; coverArt carries the full image URL so
// clients that accept URLs skip getCoverArt entirely.
pub fn playlist_from_tidal(v: &Value) -> Option<Playlist> {
    let id = v["uuid"].as_str()?.to_string();
    let name = v["title"].as_str()?.to_string();
    Some(Playlist {
        id,
        name,
        comment: v["description"].as_str().map(String::from),
        owner: v["creator"]["name"].as_str().map(String::from),
        r#public: v["publicPlaylist"].as_bool().unwrap_or(false),
        song_count: v["numberOfTracks"].as_u64().map(|n| n as u32),
        duration: v["duration"].as_u64().map(|n| n as u32),
        created: v["created"].as_str().map(String::from),
        changed: v["lastUpdated"].as_str().map(String::from),
        cover_art: v["squareImage"]
            .as_str()
            .or_else(|| v["image"].as_str())
            .map(|c| cover_url(c, 320)),
    })
}

// One entry of a playlist: the item wraps the track as { item: { ... } }.
pub fn playlist_song_from_item(v: &Value) -> Option<crate::navidrome::models::Child> {
    song_from_track(&v["item"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn playlist_item_unwraps_track() {
        let item = json!({
            "type": "track",
            "item": {
                "id": 123,
                "title": "Song One",
                "duration": 220,
                "trackNumber": 1,
                "volumeNumber": 1,
                "artists": [{"id": 9, "name": "Artist A"}],
                "album": {"id": 456, "title": "Album One", "cover": "abc-123"},
                "releaseDate": "2021-06-25"
            }
        });
        let song = playlist_song_from_item(&item).unwrap();
        assert_eq!(song.id, "t123");
        assert_eq!(song.album_id, "al456");
        assert_eq!(song.artist, "Artist A");
    }

    #[test]
    fn playlist_maps_fields() {
        let p = json!({
            "uuid": "0f31-6c0a",
            "title": "Morning Drive",
            "description": "Upbeat",
            "creator": {"id": 1, "name": "Ada"},
            "numberOfTracks": 42,
            "duration": 9134,
            "publicPlaylist": true,
            "created": "2023-01-15T10:00:00.000Z",
            "lastUpdated": "2024-02-01T08:30:00.000Z",
            "squareImage": "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"
        });
        let pl = playlist_from_tidal(&p).unwrap();
        assert_eq!(pl.id, "0f31-6c0a");
        assert_eq!(pl.owner.as_deref(), Some("Ada"));
        assert_eq!(pl.song_count, Some(42));
        assert!(pl.r#public);
        assert_eq!(
            pl.cover_art.as_deref(),
            Some("https://resources.tidal.com/images/1a2b3c4d/5e6f/4a7b/8c9d/0e1f2a3b4c5d/320x320.jpg")
        );
    }
}
