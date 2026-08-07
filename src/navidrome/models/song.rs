// Song models: the Child entry and its list wrappers.
use serde::Serialize;

// Subsonic child (song entry). Optional fields omitted when unknown.
// The OpenSubsonic schema marks contentType, suffix, size, path, created
// and isVideo as required; Feishin's song normalizer dereferences
// contentType directly, so omitting it crashes the album view.
#[derive(Serialize)]
pub struct Child {
    pub id: String,
    #[serde(rename = "parent")]
    pub parent: String,
    #[serde(rename = "isDir")]
    pub is_dir: bool,
    #[serde(rename = "isVideo")]
    pub is_video: bool,
    pub title: String,
    pub album: String,
    pub artist: String,
    #[serde(rename = "track")]
    pub track: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    pub duration: u32,
    #[serde(rename = "discNumber", skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u32>,
    #[serde(rename = "albumId")]
    pub album_id: String,
    #[serde(rename = "artistId")]
    pub artist_id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    // Placeholder media metadata: no transcode or file probe yet.
    #[serde(rename = "contentType")]
    pub content_type: &'static str,
    pub suffix: &'static str,
    pub size: u64,
    pub path: String,
    pub created: String,
    // Favorite time; present only in getStarred/getStarred2 songs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
}

// getSong data: { song: Child }
#[derive(Serialize)]
pub struct GetSongResponse {
    pub song: Child,
}

// getTopSongs data: { topSongs: { song: [ Child ] } }
#[derive(Serialize)]
pub struct TopSongsResponse {
    #[serde(rename = "topSongs")]
    pub top_songs: TopSongs,
}

#[derive(Serialize)]
pub struct TopSongs {
    pub song: Vec<Child>,
}

// getRandomSongs data: { randomSongs: { song: [ Child ] } }
#[derive(Serialize)]
pub struct RandomSongsResponse {
    #[serde(rename = "randomSongs")]
    pub random_songs: RandomSongs,
}

#[derive(Serialize)]
pub struct RandomSongs {
    pub song: Vec<Child>,
}

// getNowPlaying data: { nowPlaying: { entry: [ NowPlayingEntry ] } }. The
// entry is a full song plus the playback report fields.
#[derive(Serialize)]
pub struct NowPlayingResponse {
    #[serde(rename = "nowPlaying")]
    pub now_playing: NowPlaying,
}

#[derive(Serialize)]
pub struct NowPlaying {
    pub entry: Vec<NowPlayingEntry>,
}

#[derive(Serialize)]
pub struct NowPlayingEntry {
    #[serde(flatten)]
    pub song: Child,
    pub username: String,
    #[serde(rename = "minutesAgo")]
    pub minutes_ago: u32,
    #[serde(rename = "playerId")]
    pub player_id: u32,
}

// getSimilarSongs2 data: { similarSongs2: { song: [ Child ] } }
#[derive(Serialize)]
pub struct SimilarSongs2Response {
    #[serde(rename = "similarSongs2")]
    pub similar_songs2: SimilarSongs2,
}

#[derive(Serialize)]
pub struct SimilarSongs2 {
    pub song: Vec<Child>,
}

// getLyricsBySongId data: { lyricsList: { structuredLyrics: [...] } }.
// Version 1 shape: kind is omitted unless enhanced=true was requested.
#[derive(Serialize)]
pub struct LyricsListResponse {
    #[serde(rename = "lyricsList")]
    pub lyrics_list: LyricsList,
}

#[derive(Serialize)]
pub struct LyricsList {
    #[serde(rename = "structuredLyrics")]
    pub structured_lyrics: Vec<StructuredLyrics>,
}

#[derive(Serialize)]
pub struct StructuredLyrics {
    #[serde(rename = "displayArtist")]
    pub display_artist: String,
    #[serde(rename = "displayTitle")]
    pub display_title: String,
    pub lang: String,
    pub offset: i32,
    pub synced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    pub line: Vec<LyricLine>,
}

#[derive(Serialize)]
pub struct LyricLine {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn top_songs_wraps_song_array() {
        let resp = TopSongsResponse {
            top_songs: TopSongs { song: vec![] },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r##"{"topSongs":{"song":[]}}"##);
    }

    #[test]
    fn get_song_wraps_child() {
        let song = crate::tidal::mapping::song_from_track(&json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        }))
        .unwrap();
        let json = serde_json::to_string(&GetSongResponse { song }).unwrap();
        assert!(json.contains(r#""song":{"id":"t123""#));
    }

    #[test]
    fn now_playing_entry_flattens_song() {
        let song = crate::tidal::mapping::song_from_track(&json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        }))
        .unwrap();
        let json = serde_json::to_value(&NowPlayingEntry {
            song,
            username: "admin".into(),
            minutes_ago: 0,
            player_id: 0,
        })
        .unwrap();
        assert_eq!(json["id"], "t123");
        assert_eq!(json["username"], "admin");
        assert_eq!(json["minutesAgo"], 0);
        assert_eq!(json["playerId"], 0);
    }

    #[test]
    fn structured_lyrics_omits_kind_without_enhanced() {
        let json = serde_json::to_value(&StructuredLyrics {
            display_artist: "Muse".into(),
            display_title: "Hysteria".into(),
            lang: "eng".into(),
            offset: 0,
            synced: true,
            kind: None,
            line: vec![LyricLine {
                start: Some(0),
                value: "It's bugging me".into(),
            }],
        })
        .unwrap();
        assert!(json.get("kind").is_none());
        assert_eq!(json["line"][0]["start"], 0);
        assert_eq!(json["line"][0]["value"], "It's bugging me");
    }
}
