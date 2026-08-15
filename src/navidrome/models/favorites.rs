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

#[derive(Serialize)]
pub struct StarredAlbum {
    #[serde(flatten)]
    pub album: AlbumId3,
    pub parent: String,
    #[serde(rename = "isDir")]
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
}

// getStarred2 data: ID3 shapes. Artists carry albumCount and
// artistImageUrl (same full URL as coverArt).
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

#[derive(Serialize)]
pub struct Starred2Artist {
    #[serde(flatten)]
    pub artist: ArtistId3,
    #[serde(rename = "artistImageUrl", skip_serializing_if = "Option::is_none")]
    pub artist_image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
}

#[derive(Serialize)]
pub struct Starred2Album {
    #[serde(flatten)]
    pub album: AlbumId3,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
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
                    starred: None,
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
                        is_compilation: None,
                        release_types: None,
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
                    artist_image_url: Some("https://example.com/a.jpg".into()),
                    starred: None,
                    artist: ArtistId3 {
                        id: "ar1".into(),
                        name: "X".into(),
                        cover_art: Some("https://example.com/a.jpg".into()),
                        album_count: Some(1),
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
}
