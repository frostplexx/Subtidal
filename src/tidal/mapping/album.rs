// Map Tidal album JSON to Subsonic AlbumID3.
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::AlbumId3;

use super::{cover_url, primary_artist, year_from};

pub fn album_from_tidal(v: &Value) -> Option<AlbumId3> {
    let id = v["id"].as_u64()?;
    let name = v["title"].as_str()?.to_string();
    let (artist_id, artist_name) = primary_artist(v);
    Some(AlbumId3 {
        id: ids::encode_album(id),
        album: name.clone(),
        title: name.clone(),
        name,
        artist: artist_name,
        artist_id: ids::encode_artist(artist_id),
        cover_art: v["cover"].as_str().map(|c| cover_url(c, 640)),
        song_count: v["numberOfTracks"].as_u64().map(|n| n as u32),
        duration: v["duration"].as_u64().map(|n| n as u32),
        play_count: 0,
        created: None,
        year: year_from(v["releaseDate"].as_str()),
        genre: v["genre"].as_str().map(String::from),
    })
}

// Favorites wrap each album in { item, created }; the created date is the
// favorite time, which maps to AlbumID3.created.
pub fn favorite_album_from_tidal(entry: &Value) -> Option<AlbumId3> {
    let mut album = album_from_tidal(&entry["item"])?;
    album.created = entry["created"].as_str().map(String::from);
    Some(album)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn album_maps_fields() {
        let album = json!({
            "id": 456,
            "title": "Album One",
            "artist": {"id": 9, "name": "Artist A"},
            "numberOfTracks": 10,
            "duration": 2200,
            "releaseDate": "2020-01-01",
            "cover": "def-456"
        });
        let a = album_from_tidal(&album).unwrap();
        assert_eq!(a.id, "al456");
        assert_eq!(a.artist_id, "ar9");
        assert_eq!(a.song_count, Some(10));
        assert_eq!(a.year, Some(2020));
        assert_eq!(a.album, "Album One");
        assert_eq!(a.title, "Album One");
        assert_eq!(a.play_count, 0);
        assert_eq!(a.created, None);
    }

    #[test]
    fn favorite_album_carries_created() {
        let entry = json!({
            "item": {"id": 456, "title": "Album One", "artist": {"id": 9, "name": "Artist A"}},
            "created": "2023-01-15T10:00:00.000Z"
        });
        let a = favorite_album_from_tidal(&entry).unwrap();
        assert_eq!(a.created.as_deref(), Some("2023-01-15T10:00:00.000Z"));
    }
}
