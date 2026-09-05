// Map Tidal track JSON to Subsonic Child (song entry).
use serde_json::Value;

use crate::navidrome::ids;
use crate::navidrome::models::{ArtistRef, Child, GenreItem, ReplayGain};

use super::{
    artist_credits, content_labels, cover_url, explicit_status, lead_artist, year_from,
};

pub fn song_from_track(v: &Value) -> Option<Child> {
    let id = v["id"].as_u64()?;
    let labels = content_labels();
    let mut title = v["title"].as_str()?.to_string();
    mark_ai(&mut title, v, labels.ai);
    let album = v["album"].as_object()?;
    let album_id = album["id"].as_u64()?;
    let album_name = album["title"].as_str().unwrap_or("").to_string();
    // The legacy singular artist string and artistId carry the primary
    // artist only: the MAIN-tagged entry when Tidal marks one, else the
    // first. The OpenSubsonic artists array carries every artist.
    let (artist_id, artist_name) = lead_artist(v);
    let artists = {
        let credits = artist_credits(v);
        (!credits.is_empty()).then(|| {
            credits
                .iter()
                .map(|(id, name)| ArtistRef {
                    id: ids::encode_artist(*id),
                    name: name.clone(),
                })
                .collect()
        })
    };
    let year = year_from(v["releaseDate"].as_str())
        .or_else(|| year_from(album.get("releaseDate").and_then(|r| r.as_str())));
    // v2 flatten writes the track's first genre onto `genre` (that data
    // ships in the items.genres include); v1 track JSON embeds it on the
    // album instead. Read both, prefer the track's own.
    let genre = v["genre"].as_str().map(String::from).or_else(|| {
        album
            .get("genre")
            .and_then(|g| g.as_str())
            .map(String::from)
    });
    // The same value as the OpenSubsonic genres array.
    let genres = genre.as_ref().map(|g| vec![GenreItem { name: g.clone() }]);

    let (content_type, suffix) = format_from_track(v);

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
        artists,
        isrc: v["isrc"].as_str().map(|s| vec![s.to_string()]),
        kind: "song",
        content_type,
        suffix,
        size: 0,
        path: String::new(),
        created: String::new(),
        starred: None,
        starred_at: None,
        explicit_status: explicit_status(v, labels.explicit),
        replay_gain: ReplayGain {
            track_gain: v["replayGain"].as_f64(),
            track_peak: v["peak"].as_f64(),
        },
    })
}

// The track's own source format, from Tidal's `mediaMetadata.tags` (badge
// tags: e.g. "DOLBY_ATMOS", "HIRES_LOSSLESS", "LOSSLESS") and/or
// `audioQuality`. Both come free on a track object already fetched for
// `song_from_track` (no extra Tidal call) *when present* — the v1-sourced
// track objects backing most album/song listings (see
// `TidalClient::album_items_parallel`) carry them, but a v2-flattened
// jsonapi track (the `album_with_items` fallback path, and some search/
// mix feeds) does not, so an absent field falls back to the same lossy
// "m4a" guess this always reported before, rather than claiming a source
// format Tidal never actually told us. This mirrors a real Subsonic
// server reporting the file's own tags, not whatever a client's current
// transcode setting happens to request — so a track tagged DOLBY_ATMOS
// here still shows/streams as Atmos even for a client whose own format
// setting is plain FLAC/Off; the requested transcode format is a
// separate, later decision (see `tidal_quality` in the tracks handler).
fn format_from_track(v: &Value) -> (&'static str, &'static str) {
    let tags: &[Value] = v["mediaMetadata"]["tags"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let has_tag = |t: &str| tags.iter().any(|x| x.as_str() == Some(t));
    if has_tag("DOLBY_ATMOS") {
        return ("audio/eac3", "eac3");
    }
    if has_tag("HIRES_LOSSLESS") || v["audioQuality"].as_str() == Some("HIRES_LOSSLESS") {
        return ("audio/flac", "flac");
    }
    if has_tag("LOSSLESS") || v["audioQuality"].as_str() == Some("LOSSLESS") {
        return ("audio/flac", "flac");
    }
    ("audio/mp4", "m4a")
}

// Appends the AI marker when the track is AI-generated and the [labels]
// setting enables it.
fn mark_ai(title: &mut String, v: &Value, enabled: bool) {
    if enabled && v["ai"].as_bool() == Some(true) {
        title.push_str(" • AI");
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
        assert_eq!(song.artist, "Artist A");
        let json = serde_json::to_value(&song).unwrap();
        assert_eq!(
            json["artists"],
            json!([
                {"id": "ar9", "name": "Artist A"},
                {"id": "ar10", "name": "Artist B"}
            ])
        );
        assert_eq!(song.year, Some(2021));
        assert_eq!(song.track, 3);
        assert_eq!(song.disc_number, Some(2));
        assert_eq!(
            song.cover_art.unwrap(),
            "https://resources.tidal.com/images/abc/123/640x640.jpg"
        );
    }

    #[test]
    fn primary_artist_wins_over_array_order() {
        // v1 track JSON tags artists MAIN/FEATURED; the primary is the
        // MAIN entry even when it is not first in the array.
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [
                {"id": 10, "name": "Feat. Artist", "type": "FEATURED"},
                {"id": 9, "name": "Artist A", "type": "MAIN"}
            ],
            "album": {"id": 456, "title": "Album One"}
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.artist, "Artist A");
        assert_eq!(song.artist_id, "ar9");
        // The artists array keeps the original order, not the primary.
        let json = serde_json::to_value(&song).unwrap();
        assert_eq!(json["artists"][0]["id"], "ar10");
        assert_eq!(json["artists"][1]["id"], "ar9");
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
    fn song_maps_isrc() {
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "isrc": "USYT22100001"
        });
        let song = song_from_track(&track).unwrap();
        let json = serde_json::to_value(&song).unwrap();
        assert_eq!(json["isrc"], json!(["USYT22100001"]));

        // Payloads without an ISRC emit no isrc key at all.
        let bare = json!({
            "id": 124,
            "title": "Song Two",
            "duration": 220,
            "trackNumber": 4,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        });
        let song = song_from_track(&bare).unwrap();
        let json = serde_json::to_value(&song).unwrap();
        assert!(json.get("isrc").is_none());
    }

    #[test]
    fn song_falls_back_to_m4a_when_no_quality_metadata_present() {
        // A v2-flattened track (the album_with_items fallback, some search/
        // mix feeds) carries neither mediaMetadata.tags nor audioQuality —
        // keep the old lossy-container guess rather than claiming a format
        // Tidal never actually told us.
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
    fn song_maps_dolby_atmos_tag_to_eac3() {
        let track = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "audioQuality": "LOSSLESS",
            "mediaMetadata": {"tags": ["LOSSLESS", "DOLBY_ATMOS"]}
        });
        let song = song_from_track(&track).unwrap();
        assert_eq!(song.content_type, "audio/eac3");
        assert_eq!(song.suffix, "eac3");
    }

    #[test]
    fn song_maps_hires_lossless_and_lossless_tags_to_flac() {
        let hires = json!({
            "id": 123, "title": "Song One", "duration": 220, "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "mediaMetadata": {"tags": ["HIRES_LOSSLESS"]}
        });
        let song = song_from_track(&hires).unwrap();
        assert_eq!(song.content_type, "audio/flac");
        assert_eq!(song.suffix, "flac");

        let lossless_via_audio_quality = json!({
            "id": 124, "title": "Song Two", "duration": 220, "trackNumber": 4,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "audioQuality": "LOSSLESS"
        });
        let song = song_from_track(&lossless_via_audio_quality).unwrap();
        assert_eq!(song.content_type, "audio/flac");
        assert_eq!(song.suffix, "flac");
    }

    #[test]
    fn song_keeps_lossy_container_for_high_and_low_quality() {
        let high = json!({
            "id": 123, "title": "Song One", "duration": 220, "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "audioQuality": "HIGH"
        });
        let song = song_from_track(&high).unwrap();
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

    #[test]
    fn song_maps_explicit_status() {
        let explicit = json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "explicit": true
        });
        let song = song_from_track(&explicit).unwrap();
        assert_eq!(song.explicit_status.as_deref(), Some("explicit"));
        let json = serde_json::to_value(&song).unwrap();
        assert_eq!(json["explicitStatus"], "explicit");

        let clean = json!({
            "id": 124,
            "title": "Song Two",
            "duration": 220,
            "trackNumber": 4,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "explicit": false
        });
        let song = song_from_track(&clean).unwrap();
        assert_eq!(song.explicit_status.as_deref(), Some("clean"));

        // Endpoints that omit the flag produce no explicitStatus at all.
        let bare = json!({
            "id": 125,
            "title": "Song Three",
            "duration": 220,
            "trackNumber": 5,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        });
        let song = song_from_track(&bare).unwrap();
        assert_eq!(song.explicit_status, None);
        let json = serde_json::to_value(&song).unwrap();
        assert!(json.get("explicitStatus").is_none());
    }

    #[test]
    fn ai_marker_obeys_label_setting() {
        let ai = json!({"ai": true});
        let mut title = "Song One".to_string();
        mark_ai(&mut title, &ai, true);
        assert_eq!(title, "Song One • AI");
        let mut title = "Song One".to_string();
        mark_ai(&mut title, &ai, false);
        assert_eq!(title, "Song One");
        let not_ai = json!({"ai": false});
        let mut title = "Song One".to_string();
        mark_ai(&mut title, &not_ai, true);
        assert_eq!(title, "Song One");
    }

    #[test]
    fn explicit_status_omitted_when_labels_off() {
        let explicit = json!({"explicit": true});
        assert_eq!(explicit_status(&explicit, false), None);
        assert_eq!(
            explicit_status(&explicit, true),
            Some("explicit".to_string())
        );
    }
}
