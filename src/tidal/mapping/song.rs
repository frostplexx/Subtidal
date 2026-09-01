// Map Tidal track JSON to Subsonic Child (song entry).
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::{Child, GenreItem, ReplayGain};

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
    // v2 flatten writes the track's first genre onto `genre` (that data
    // ships in the items.genres include); v1 track JSON embeds it on the
    // album instead. Read both, prefer the track's own.
    let genre = v["genre"]
        .as_str()
        .map(String::from)
        .or_else(|| album.get("genre").and_then(|g| g.as_str()).map(String::from));
    // The same value as the OpenSubsonic genres array.
    let genres = genre
        .as_ref()
        .map(|g| vec![GenreItem { name: g.clone() }]);
    // v2 tracks carry no audioQuality; every stream is served as an HLS
    // playlist of MP4 segments, so the container is always m4a.
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
        genre,
        genres,
        cover_art: album
            .get("cover")
            .and_then(|c| c.as_str())
            .map(|c| cover_url(c, 640)),
        duration: v["duration"].as_u64().unwrap_or(0) as u32,
        disc_number: v["volumeNumber"].as_u64().map(|n| n as u32),
        album_id: ids::encode_album(album_id),
        artist_id: ids::encode_artist(artist_id),
        kind: "song",
        content_type: "audio/mp4",
        suffix: "m4a",
        size: 0,
        path: String::new(),
        created: String::new(),
        starred: None,
        starred_at: None,
        replay_gain: ReplayGain {
            track_gain: v["replayGain"].as_f64(),
            track_peak: v["peak"].as_f64(),
        },
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

    #[test]
    fn song_takes_genre_from_the_track_when_flattened() {
        // The v2 flatten writes the track's own first genre onto `genre`;
        // the embedded v1 album object carries album.genre instead.
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "genre": "Hip-Hop",
            "album": {"id": 456, "title": "Album One", "genre": "Rock"}
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.genre.as_deref(), Some("Hip-Hop"));
    }

    #[test]
    fn song_takes_genre_from_the_album_when_track_has_none() {
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One", "genre": "Rock"}
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.genre.as_deref(), Some("Rock"));
    }

    #[test]
    fn song_maps_container_to_m4a() {
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.content_type, "audio/mp4");
        assert_eq!(song.suffix, "m4a");
    }

    #[test]
    fn song_maps_replay_gain() {
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "replayGain": -6.2,
            "peak": 0.85
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.replay_gain.track_gain, Some(-6.2));
        assert_eq!(song.replay_gain.track_peak, Some(0.85));
    }

    #[test]
    fn song_omits_replay_gain_when_absent() {
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.replay_gain.track_gain, None);
        assert_eq!(song.replay_gain.track_peak, None);
    }
}
