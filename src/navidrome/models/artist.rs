// Artist models: the ID3 artist and the artist-info response.
use serde::Serialize;

use super::album::AlbumId3;

// getArtist data: { artist: ArtistWithAlbumsID3 }
#[derive(Serialize)]
pub struct GetArtistResponse {
    pub artist: ArtistWithAlbums,
}

// ArtistID3 plus its albums. The artist fields flatten from ArtistId3;
// albumCount is the number of albums actually returned.
#[derive(Serialize)]
pub struct ArtistWithAlbums {
    #[serde(flatten)]
    pub artist: ArtistId3,
    pub album: Vec<AlbumId3>,
}

// Legacy Artist element (search2): ArtistID3 minus albumCount when absent.
#[derive(Serialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(rename = "albumCount", skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u32>,
}

impl From<&ArtistId3> for Artist {
    fn from(a: &ArtistId3) -> Self {
        Artist {
            id: a.id.clone(),
            name: a.name.clone(),
            cover_art: a.cover_art.clone(),
            album_count: a.album_count,
        }
    }
}

#[derive(Serialize)]
pub struct ArtistId3 {
    pub id: String,
    pub name: String,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(rename = "albumCount", skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u32>,
}

// getArtistInfo data: { artistInfo: ArtistInfoID3 }. Same payload as
// getArtistInfo2, different wrapper name.
#[derive(Serialize)]
pub struct ArtistInfoResponse {
    #[serde(rename = "artistInfo")]
    pub artist_info: ArtistInfo2,
}

// getArtistInfo2 data: { artistInfo2: ArtistInfoID3 }. Images are the artist
// portrait at the three documented sizes; musicBrainzId is empty until Tidal
// exposes it (artist detail carries no external links today).
#[derive(Serialize)]
pub struct ArtistInfo2Response {
    #[serde(rename = "artistInfo2")]
    pub artist_info: ArtistInfo2,
}

#[derive(Serialize)]
pub struct ArtistInfo2 {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub biography: String,
    #[serde(rename = "musicBrainzId", skip_serializing_if = "String::is_empty")]
    pub music_brainz_id: String,
    #[serde(rename = "lastFmUrl", skip_serializing_if = "String::is_empty")]
    pub last_fm_url: String,
    #[serde(rename = "smallImageUrl", skip_serializing_if = "Option::is_none")]
    pub small_image_url: Option<String>,
    #[serde(rename = "mediumImageUrl", skip_serializing_if = "Option::is_none")]
    pub medium_image_url: Option<String>,
    #[serde(rename = "largeImageUrl", skip_serializing_if = "Option::is_none")]
    pub large_image_url: Option<String>,
    #[serde(rename = "similarArtist", skip_serializing_if = "Vec::is_empty")]
    pub similar_artist: Vec<ArtistId3>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artist_info2_omits_empty_and_unknown_fields() {
        let resp = ArtistInfo2Response {
            artist_info: ArtistInfo2 {
                biography: "A band.".into(),
                music_brainz_id: String::new(),
                last_fm_url: String::new(),
                small_image_url: Some("https://example.com/s.jpg".into()),
                medium_image_url: None,
                large_image_url: None,
                similar_artist: vec![],
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        let info = &json["artistInfo2"];
        assert_eq!(info["biography"], "A band.");
        assert_eq!(info["smallImageUrl"], "https://example.com/s.jpg");
        assert!(info.get("musicBrainzId").is_none());
        assert!(info.get("largeImageUrl").is_none());
        assert!(info.get("similarArtist").is_none());
    }

    #[test]
    fn artist_info_wraps_under_artist_info_name() {
        let resp = ArtistInfoResponse {
            artist_info: ArtistInfo2 {
                biography: "A band.".into(),
                music_brainz_id: String::new(),
                last_fm_url: String::new(),
                small_image_url: None,
                medium_image_url: None,
                large_image_url: None,
                similar_artist: vec![],
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["artistInfo"]["biography"], "A band.");
        assert!(json.get("artistInfo2").is_none());
    }

    #[test]
    fn artist_with_albums_flattens_artist_fields() {
        let resp = GetArtistResponse {
            artist: ArtistWithAlbums {
                artist: ArtistId3 {
                    id: "ar1".into(),
                    name: "X".into(),
                    cover_art: Some("https://example.com/a.jpg".into()),
                    album_count: Some(2),
                },
                album: vec![AlbumId3 {
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
                }],
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["artist"]["id"], "ar1");
        assert_eq!(json["artist"]["albumCount"], 2);
        assert_eq!(json["artist"]["album"][0]["id"], "al1");
    }
}
