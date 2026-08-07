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
}
