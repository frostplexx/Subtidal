// Map Tidal track JSON to Subsonic Child (song entry).
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::{Child, ReplayGain};

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
    let (content_type, suffix) = container(v["audioQuality"].as_str());
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
        content_type,
        suffix,
        size: 0,
        path: String::new(),
        created: String::new(),
        starred: None,
        replay_gain: ReplayGain {
            track_gain: v["replayGain"].as_f64(),
            track_peak: v["peak"].as_f64(),
        },
    })
}

// The stream container follows the account's quality tier: lossless
// tiers stream FLAC, the others AAC in an MP4 container. The tier in the
// track JSON is the only signal available without a stream fetch; the
// manifest mimeType (Sone reads it too) would be exact but costs one
// playbackinfo call per track.
fn container(quality: Option<&str>) -> (&'static str, &'static str) {
    match quality {
        Some("LOSSLESS") | Some("HIRES_LOSSLESS") => ("audio/flac", "flac"),
        _ => ("audio/mp4", "m4a"),
    }
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
    fn container_follows_quality_tier() {
        assert_eq!(container(Some("LOSSLESS")), ("audio/flac", "flac"));
        assert_eq!(container(Some("HIRES_LOSSLESS")), ("audio/flac", "flac"));
        assert_eq!(container(Some("HIGH")), ("audio/mp4", "m4a"));
        assert_eq!(container(Some("LOW")), ("audio/mp4", "m4a"));
        assert_eq!(container(None), ("audio/mp4", "m4a"));
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

    #[test]
    fn song_maps_container_from_quality() {
        let track = json!({
            "id": 123,
            "title": "Song One",
            "audioQuality": "HIGH",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.content_type, "audio/mp4");
        assert_eq!(song.suffix, "m4a");
    }
}
