// Map Tidal album JSON to Subsonic AlbumID3.
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::{AlbumId3, GenreItem};

use super::{cover_url, primary_artist, year_from};

pub fn album_from_tidal(v: &Value) -> Option<AlbumId3> {
    let id = v["id"].as_u64()?;
    let name = v["title"].as_str()?.to_string();
    let (artist_id, artist_name) = primary_artist(v);
    // Compilations are stamped by the artist client (Tidal's own item
    // type cannot distinguish them); everything else maps its release
    // type from Tidal's `type` field.
    let is_compilation = v["isCompilation"].as_bool().unwrap_or(false);
    let release_types = if is_compilation {
        vec!["Compilation".to_string()]
    } else {
        match v["type"].as_str() {
            Some("ALBUM") => vec!["Album".to_string()],
            Some("EP") => vec!["EP".to_string()],
            Some("SINGLE") => vec!["Single".to_string()],
            _ => Vec::new(),
        }
    };
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
        genres: v["genre"].as_str().map(|g| vec![GenreItem { name: g.to_string() }]),
        explicit_status: v["explicit"]
            .as_bool()
            .map(|e| if e { "explicit" } else { "clean" }.to_string()),
        is_compilation: is_compilation.then_some(true),
        release_types: (!release_types.is_empty()).then_some(release_types),
        starred: None,
        starred_at: None,
    })
}

// Favorites wrap each album in { item, created }; the created date is the
// favorite time, which maps to AlbumID3.created, starred, and starredAt.
pub fn favorite_album_from_tidal(entry: &Value) -> Option<AlbumId3> {
    let mut album = album_from_tidal(&entry["item"])?;
    album.created = entry["created"].as_str().map(String::from);
    album.starred = entry["created"].as_str().map(String::from);
    album.starred_at = entry["created"].as_str().map(String::from);
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
            "cover": "def-456",
            "type": "ALBUM",
            "explicit": true
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
        assert_eq!(a.release_types, Some(vec!["Album".to_string()]));
        assert_eq!(a.explicit_status.as_deref(), Some("explicit"));
        assert_eq!(a.is_compilation, None);
    }

    #[test]
    fn album_maps_ep_single_and_compilation_types() {
        let ep = album_from_tidal(&json!({"id": 1, "title": "E", "type": "EP"})).unwrap();
        assert_eq!(ep.release_types, Some(vec!["EP".to_string()]));
        assert_eq!(ep.is_compilation, None);
        let single = album_from_tidal(&json!({"id": 2, "title": "S", "type": "SINGLE"})).unwrap();
        assert_eq!(single.release_types, Some(vec!["Single".to_string()]));
        // A compilation is stamped by the artist client; its own type
        // field cannot distinguish it from an album.
        let comp = album_from_tidal(&json!({
            "id": 3,
            "title": "C",
            "type": "ALBUM",
            "isCompilation": true
        }))
        .unwrap();
        assert_eq!(comp.release_types, Some(vec!["Compilation".to_string()]));
        assert_eq!(comp.is_compilation, Some(true));
        // Missing type: no releaseTypes.
        let bare = album_from_tidal(&json!({"id": 4, "title": "B"})).unwrap();
        assert_eq!(bare.release_types, None);
        assert_eq!(bare.is_compilation, None);
    }

    #[test]
    fn favorite_album_carries_created() {
        let entry = json!({
            "item": {"id": 456, "title": "Album One", "artist": {"id": 9, "name": "Artist A"}},
            "created": "2023-01-15T10:00:00.000Z"
        });
        let a = favorite_album_from_tidal(&entry).unwrap();
        assert_eq!(a.created.as_deref(), Some("2023-01-15T10:00:00.000Z"));
        assert_eq!(a.starred.as_deref(), Some("2023-01-15T10:00:00.000Z"));
        assert_eq!(a.starred_at.as_deref(), Some("2023-01-15T10:00:00.000Z"));
    }
}
