// Favorites (starred) response models. getStarred uses legacy shapes;
// getStarred2 the ID3 shapes. Both wrap the same three entity lists.
use serde::Serialize;

use super::album::AlbumId3;
use super::artist::ArtistId3;
use super::song::Child;

// getStarred data: legacy shapes. Albums get a parent (the artist id) and
// isDir; artists stay minimal.
#[derive(Serialize)]
pub struct StarredResponse {
    pub starred: Starred,
}

#[derive(Serialize)]
pub struct Starred {
    pub artist: Vec<StarredArtist>,
    pub album: Vec<StarredAlbum>,
    pub song: Vec<Child>,
}

#[derive(Serialize)]
pub struct StarredArtist {
    pub id: String,
    pub name: String,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
}

// `starred`/`starredAt` come from the flattened AlbumId3, already set by
// favorite_album_from_tidal; a duplicate field here would serialize the
// key twice.
#[derive(Serialize)]
pub struct StarredAlbum {
    #[serde(flatten)]
    pub album: AlbumId3,
    pub parent: String,
    #[serde(rename = "isDir")]
    pub is_dir: bool,
}

// getStarred2 data: ID3 shapes. Artists are plain ArtistId3 (albumCount
// and artistImageUrl included) plus the favorite time.
#[derive(Serialize)]
pub struct Starred2Response {
    pub starred2: Starred2,
}

#[derive(Serialize)]
pub struct Starred2 {
    pub artist: Vec<Starred2Artist>,
    pub album: Vec<Starred2Album>,
    pub song: Vec<Child>,
}

// `starred`/`starredAt`/`artistImageUrl` come from the flattened ArtistId3;
// a duplicate field here would serialize the key twice.
#[derive(Serialize)]
pub struct Starred2Artist {
    #[serde(flatten)]
    pub artist: ArtistId3,
}

// `starred`/`starredAt` come from the flattened AlbumId3, already set by
// favorite_album_from_tidal; a duplicate field here would serialize the
// key twice.
#[derive(Serialize)]
pub struct Starred2Album {
    #[serde(flatten)]
    pub album: AlbumId3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starred_and_starred2_shapes() {
        let starred = StarredResponse {
            starred: Starred {
                artist: vec![StarredArtist {
                    id: "ar1".into(),
                    name: "X".into(),
                    cover_art: Some("https://example.com/a.jpg".into()),
                    starred: Some("2026-05-31T11:07:08Z".into()),
                }],
                album: vec![StarredAlbum {
                    parent: "ar1".into(),
                    is_dir: true,
                    album: AlbumId3 {
                        id: "al1".into(),
                        album: "A".into(),
                        title: "A".into(),
                        name: "A".into(),
                        artist: "X".into(),
                        artist_id: "ar1".into(),
                        cover_art: None,
                        song_count: None,
                        duration: None,
                        play_count: 0,
                        created: None,
                        year: None,
                        genre: None,
                        genres: None,
                        is_compilation: None,
                        release_types: None,
                        starred: None,
                        starred_at: None,
                        explicit_status: None,
                    },
                }],
                song: vec![],
            },
        };
        let json = serde_json::to_value(&starred).unwrap();
        assert_eq!(json["starred"]["artist"][0]["coverArt"], "https://example.com/a.jpg");
        assert_eq!(json["starred"]["album"][0]["parent"], "ar1");
        assert_eq!(json["starred"]["album"][0]["isDir"], true);

        let starred2 = Starred2Response {
            starred2: Starred2 {
                artist: vec![Starred2Artist {
                    artist: ArtistId3 {
                        id: "ar1".into(),
                        name: "X".into(),
                        cover_art: Some("https://example.com/a.jpg".into()),
                        artist_image_url: Some("https://example.com/a.jpg".into()),
                        album_count: Some(1),
                        sort_name: "X".into(),
                        roles: vec![],
                        starred: None,
                        starred_at: None,
                    },
                }],
                album: vec![],
                song: vec![],
            },
        };
        let json = serde_json::to_value(&starred2).unwrap();
        assert_eq!(json["starred2"]["artist"][0]["albumCount"], 1);
        assert_eq!(
            json["starred2"]["artist"][0]["artistImageUrl"],
            "https://example.com/a.jpg"
        );
    }

    // Regression: Starred2Artist/Starred2Album/StarredAlbum used to carry
    // their own `starred` field alongside the flattened ArtistId3/AlbumId3,
    // which already has one. serde_json::to_value collapses duplicate keys
    // into a Map (last write wins), so only the raw serialized string
    // reveals a literal repeated key — which some strict JSON decoders
    // (this bit a real client) reject outright.
    #[test]
    fn flattened_entities_serialize_starred_exactly_once() {
        let artist = Starred2Artist {
            artist: ArtistId3 {
                id: "ar1".into(),
                name: "X".into(),
                cover_art: None,
                artist_image_url: Some("https://example.com/a.jpg".into()),
                album_count: None,
                sort_name: "X".into(),
                roles: vec![],
                starred: Some("2026-05-31T11:07:08Z".into()),
                starred_at: Some("2026-05-31T11:07:08Z".into()),
            },
        };
        let raw = serde_json::to_string(&artist).unwrap();
        assert_eq!(raw.matches("\"starred\"").count(), 1);
        assert_eq!(raw.matches("\"starredAt\"").count(), 1);
        assert_eq!(raw.matches("\"artistImageUrl\"").count(), 1);
    }
}
