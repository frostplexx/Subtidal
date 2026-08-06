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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_songs_wraps_song_array() {
        let resp = TopSongsResponse {
            top_songs: TopSongs { song: vec![] },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r##"{"topSongs":{"song":[]}}"##);
    }
}
