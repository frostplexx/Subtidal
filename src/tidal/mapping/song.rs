// Map Tidal track JSON to Subsonic Child (song entry).
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::Child;

use super::{cover_url, primary_artist, year_from};

pub fn song_from_track(v: &Value) -> Option<Child> {
    let id = v["id"].as_u64()?;
    let title = v["title"].as_str()?.to_string();
    let album = v["album"].as_object()?;
    let album_id = album["id"].as_u64()?;
    let album_name = album["title"].as_str().unwrap_or("").to_string();
    let (artist_id, artist_name) = primary_artist(v);
    let year = year_from(v["releaseDate"].as_str())
        .or_else(|| year_from(album.get("releaseDate").and_then(|r| r.as_str())));
    Some(Child {
        id: ids::encode_track(id),
        parent: ids::encode_album(album_id),
        is_dir: false,
        is_video: false,
        title,
        album: album_name,
        artist: artist_name,
        track: v["trackNumber"].as_u64().unwrap_or(0) as u32,
        year,
        genre: album
            .get("genre")
            .and_then(|g| g.as_str())
            .map(String::from),
        cover_art: album
            .get("cover")
            .and_then(|c| c.as_str())
            .map(|c| cover_url(c, 640)),
        duration: v["duration"].as_u64().unwrap_or(0) as u32,
        disc_number: v["volumeNumber"].as_u64().map(|n| n as u32),
        album_id: ids::encode_album(album_id),
        artist_id: ids::encode_artist(artist_id),
        kind: "song",
        // Tidal streams lossless FLAC by default; the real container is
        // only known once streaming lands, so this stays a placeholder.
        content_type: "audio/flac",
        suffix: "flac",
        size: 0,
        path: String::new(),
        created: String::new(),
        starred: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn song_maps_fields() {
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "volumeNumber": 2,
            "artists": [{"id": 9, "name": "Artist A"}, {"id": 10, "name": "Artist B"}],
            "album": {"id": 456, "title": "Album One", "cover": "abc-123"},
            "releaseDate": "2021-06-25"
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.id, "t123");
        assert_eq!(song.album_id, "al456");
        assert_eq!(song.artist_id, "ar9");
        assert_eq!(song.artist, "Artist A feat. Artist B");
        assert_eq!(song.year, Some(2021));
        assert_eq!(song.track, 3);
        assert_eq!(song.disc_number, Some(2));
        assert_eq!(
            song.cover_art.unwrap(),
            "https://resources.tidal.com/images/abc/123/640x640.jpg"
        );
    }
}
