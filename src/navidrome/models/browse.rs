// Browsing models: the library index (getIndexes/getArtists) and the
// directory listing (getMusicDirectory).
use serde::Serialize;

use super::song::{Child, GenreItem};

// getIndexes data: { indexes: Indexes }
#[derive(Serialize)]
pub struct IndexesResponse {
    pub indexes: Indexes,
}

#[derive(Serialize)]
pub struct Indexes {
    #[serde(rename = "ignoredArticles")]
    pub ignored_articles: &'static str,
    pub index: Vec<IndexGroup>,
    // Virtual-folder shortcuts and root-level songs: this server has
    // neither, so both stay empty and are omitted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shortcut: Vec<IndexShortcut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub child: Vec<DirectoryChild>,
    #[serde(rename = "lastModified")]
    pub last_modified: i64,
}

// getArtists data: { artists: Artists }
#[derive(Serialize)]
pub struct ArtistsResponse {
    pub artists: Artists,
}

#[derive(Serialize)]
pub struct Artists {
    #[serde(rename = "ignoredArticles")]
    pub ignored_articles: &'static str,
    pub index: Vec<IndexGroup>,
}

// One letter group of the index: index["A"] = { name: "A", artist: [...] }
#[derive(Serialize)]
pub struct IndexGroup {
    pub name: String,
    pub artist: Vec<IndexArtist>,
}

// An artist inside the index. getIndexes carries the favorite time in
// starred; getArtists serves the same shape without it.
#[derive(Serialize)]
pub struct IndexArtist {
    pub id: String,
    pub name: String,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(rename = "albumCount", skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
}

// Virtual-folder shortcuts (podcasts, audiobooks). This server has none,
// so the list stays empty and is omitted.
#[derive(Serialize)]
pub struct IndexShortcut {
    pub id: String,
    pub name: String,
}

// getMusicDirectory data: { directory: Directory }
#[derive(Serialize)]
pub struct DirectoryResponse {
    pub directory: Directory,
}

#[derive(Serialize)]
pub struct Directory {
    pub id: String,
    pub name: String,
    pub child: Vec<DirectoryChild>,
}

// One directory entry: a subdirectory (artist or album) or a song. Both
// share the element type; songs fill the media fields, directories stay
// minimal.
#[derive(Serialize)]
pub struct DirectoryChild {
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
    #[serde(rename = "track", skip_serializing_if = "Option::is_none")]
    pub track: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<GenreItem>>,
    #[serde(rename = "explicitStatus", skip_serializing_if = "Option::is_none")]
    pub explicit_status: Option<String>,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    #[serde(rename = "discNumber", skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u32>,
    #[serde(rename = "albumId", skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(rename = "artistId", skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suffix: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(rename = "songCount", skip_serializing_if = "Option::is_none")]
    pub song_count: Option<u32>,
    // OpenSubsonic ReplayGain, same shape as a song child so the player
    // applies normalization also when the queue came from a directory.
    #[serde(rename = "replayGain", skip_serializing_if = "Option::is_none")]
    pub replay_gain: Option<crate::navidrome::models::ReplayGain>,
}

// A song fills every field; a directory stays minimal.
impl From<Child> for DirectoryChild {
    fn from(s: Child) -> Self {
        DirectoryChild {
            id: s.id,
            parent: s.parent,
            is_dir: s.is_dir,
            is_video: s.is_video,
            title: s.title,
            album: s.album,
            artist: s.artist,
            track: Some(s.track),
            year: s.year,
            genre: s.genre,
            genres: s.genres,
            explicit_status: s.explicit_status,
            cover_art: s.cover_art,
            duration: Some(s.duration),
            disc_number: s.disc_number,
            album_id: Some(s.album_id),
            artist_id: Some(s.artist_id),
            kind: Some(s.kind),
            content_type: Some(s.content_type),
            suffix: Some(s.suffix),
            size: Some(s.size),
            path: Some(s.path),
            created: Some(s.created),
            song_count: None,
            replay_gain: Some(s.replay_gain),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn indexes_shape_omits_empty_lists() {
        let resp = IndexesResponse {
            indexes: Indexes {
                ignored_articles: "The El La Los Las Le Les",
                index: vec![IndexGroup {
                    name: "A".into(),
                    artist: vec![IndexArtist {
                        id: "ar1".into(),
                        name: "ABBA".into(),
                        cover_art: Some("https://example.com/a.jpg".into()),
                        album_count: Some(2),
                        starred: Some("2023-01-15T10:00:00.000Z".into()),
                    }],
                }],
                shortcut: vec![],
                child: vec![],
                last_modified: 1_673_776_800_000,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        let idx = &json["indexes"];
        assert_eq!(idx["ignoredArticles"], "The El La Los Las Le Les");
        assert_eq!(idx["index"][0]["name"], "A");
        assert_eq!(
            idx["index"][0]["artist"][0]["coverArt"],
            "https://example.com/a.jpg"
        );
        assert_eq!(idx["index"][0]["artist"][0]["albumCount"], 2);
        assert_eq!(idx["lastModified"], 1_673_776_800_000i64);
        assert!(idx.get("shortcut").is_none());
        assert!(idx.get("child").is_none());
    }

    #[test]
    fn artists_shape_has_no_last_modified() {
        let resp = ArtistsResponse {
            artists: Artists {
                ignored_articles: "The El La Los Las Le Les",
                index: vec![],
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        let artists = &json["artists"];
        assert_eq!(artists["ignoredArticles"], "The El La Los Las Le Les");
        assert_eq!(artists["index"], json!([]));
        assert!(artists.get("lastModified").is_none());
    }

    #[test]
    fn directory_child_from_song_keeps_fields() {
        let song = crate::tidal::mapping::song_from_track(&json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        }))
        .unwrap();
        let json = serde_json::to_value(DirectoryChild::from(song)).unwrap();
        assert_eq!(json["id"], "t123");
        assert_eq!(json["parent"], "al456");
        assert_eq!(json["isDir"], false);
        assert_eq!(json["isVideo"], false);
        assert_eq!(json["type"], "song");
        assert_eq!(json["contentType"], "audio/mp4");
        assert_eq!(json["track"], 3);
        assert_eq!(json["albumId"], "al456");
        assert_eq!(json["artistId"], "ar9");
    }
}
