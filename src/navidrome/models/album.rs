// Album models: the ID3 album, its wrappers, and the song-ful variant.
use serde::Serialize;

use super::song::{Child, GenreItem};

// getAlbumList2 data: { albumList2: { album: [ AlbumID3 ] } }
// The wrapper object with the `album` array matches the Subsonic spec.
#[derive(Serialize)]
pub struct AlbumList2Response {
    #[serde(rename = "albumList2")]
    pub album_list: AlbumList2,
}

#[derive(Serialize)]
pub struct AlbumList2 {
    pub album: Vec<AlbumId3>,
}

// getAlbum data: { album: AlbumID3WithSongs }
#[derive(Serialize)]
pub struct GetAlbumResponse {
    pub album: AlbumWithSongs,
}

// AlbumID3 plus its tracks. The album fields flatten from AlbumId3, so the
// two share one field set; `song` carries the tracks in track order.
#[derive(Serialize)]
pub struct AlbumWithSongs {
    #[serde(flatten)]
    pub album: AlbumId3,
    pub song: Vec<Child>,
}

// getAlbumList data: { albumList: { album: [ Album ] } } (v1 legacy shapes)
#[derive(Serialize)]
pub struct AlbumListResponse {
    #[serde(rename = "albumList")]
    pub album_list: AlbumList,
}

#[derive(Serialize)]
pub struct AlbumList {
    pub album: Vec<Album>,
}

// Legacy Album element (getAlbumList v1, search2): AlbumID3 without the
// name aliases.
#[derive(Serialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: String,
    #[serde(rename = "artistId")]
    pub artist_id: String,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(rename = "songCount", skip_serializing_if = "Option::is_none")]
    pub song_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    #[serde(rename = "playCount")]
    pub play_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<GenreItem>>,
}

impl From<&AlbumId3> for Album {
    fn from(a: &AlbumId3) -> Self {
        Album {
            id: a.id.clone(),
            name: a.name.clone(),
            artist: a.artist.clone(),
            artist_id: a.artist_id.clone(),
            cover_art: a.cover_art.clone(),
            song_count: a.song_count,
            duration: a.duration,
            play_count: a.play_count,
            created: a.created.clone(),
            year: a.year,
            genre: a.genre.clone(),
            genres: a.genres.clone(),
        }
    }
}

// getAlbumInfo / getAlbumInfo2 data. Tidal has no album notes and no
// external ids, so only the artwork is real; notes, musicBrainzId, and
// lastFmUrl stay empty and are omitted.
#[derive(Serialize)]
pub struct AlbumInfoResponse {
    #[serde(rename = "albumInfo")]
    pub album_info: AlbumInfo,
}

#[derive(Serialize)]
pub struct AlbumInfo2Response {
    #[serde(rename = "albumInfo2")]
    pub album_info: AlbumInfo,
}

#[derive(Serialize)]
pub struct AlbumInfo {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
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
}

#[derive(Serialize)]
pub struct AlbumId3 {
    pub id: String,
    // Legacy aliases; the documented response repeats the name in all three.
    pub album: String,
    pub title: String,
    pub name: String,
    pub artist: String,
    #[serde(rename = "artistId")]
    pub artist_id: String,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(rename = "songCount", skip_serializing_if = "Option::is_none")]
    pub song_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    // No play tracking yet, so this is always 0, as in the documented example.
    #[serde(rename = "playCount")]
    pub play_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<GenreItem>>,
    // OpenSubsonic explicitStatus mirrors the release's `explicit` flag;
    // omitted when the API payload carries no flag.
    #[serde(rename = "explicitStatus", skip_serializing_if = "Option::is_none")]
    pub explicit_status: Option<String>,
    // OpenSubsonic extensions: releaseTypes carries Album/EP/Single, and
    // isCompilation marks guest-appearance compilations. Both omit when
    // unknown, matching the codebase's skip-when-none convention.
    #[serde(rename = "isCompilation", skip_serializing_if = "Option::is_none")]
    pub is_compilation: Option<bool>,
    #[serde(rename = "releaseTypes", skip_serializing_if = "Option::is_none")]
    pub release_types: Option<Vec<String>>,
    // Favorite time (OpenSubsonic 1.16.5 renamed the deprecated `starred`
    // to `starredAt`); present only on albums from favorites lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
    #[serde(rename = "starredAt", skip_serializing_if = "Option::is_none")]
    pub starred_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_list2_wraps_album_array() {
        let resp = AlbumList2Response {
            album_list: AlbumList2 {
                album: vec![AlbumId3 {
                    id: "al1".into(),
                    album: "A".into(),
                    title: "A".into(),
                    name: "A".into(),
                    artist: "X".into(),
                    artist_id: "ar1".into(),
                    cover_art: None,
                    song_count: Some(20),
                    duration: Some(4248),
                    play_count: 0,
                    created: Some("2021-07-22T02:09:31+00:00".into()),
                    year: Some(2005),
                    genre: Some("Hip-Hop".into()),
                    genres: Some(vec![GenreItem { name: "Hip-Hop".into() }]),
                    is_compilation: None,
                    release_types: None,
                    starred: None,
                    starred_at: None,
                    explicit_status: None,
                }],
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r##"{"albumList2":{"album":[{"id":"al1","album":"A","title":"A","name":"A","artist":"X","artistId":"ar1","songCount":20,"duration":4248,"playCount":0,"created":"2021-07-22T02:09:31+00:00","year":2005,"genre":"Hip-Hop","genres":[{"name":"Hip-Hop"}]}]}}"##
        );
    }

    #[test]
    fn album_with_songs_flattens_album_fields() {
        let resp = GetAlbumResponse {
            album: AlbumWithSongs {
                album: AlbumId3 {
                    id: "al1".into(),
                    album: "A".into(),
                    title: "A".into(),
                    name: "A".into(),
                    artist: "X".into(),
                    artist_id: "ar1".into(),
                    cover_art: Some("https://example.com/c.jpg".into()),
                    song_count: Some(1),
                    duration: Some(200),
                    play_count: 0,
                    created: None,
                    year: Some(2020),
                    genre: None,
                    genres: None,
                    is_compilation: None,
                    release_types: None,
                    starred: None,
                    starred_at: None,
                    explicit_status: None,
                },
                song: vec![Child {
                    id: "t9".into(),
                    parent: "al1".into(),
                    is_dir: false,
                    is_video: false,
                    title: "S".into(),
                    album: "A".into(),
                    artist: "X".into(),
                    track: 1,
                    year: Some(2020),
                    genre: None,
                    genres: None,
                    cover_art: None,
                    duration: 200,
                    disc_number: None,
                    album_id: "al1".into(),
                    artist_id: "ar1".into(),
                    kind: "song",
                    content_type: "audio/flac",
                    suffix: "flac",
                    size: 0,
                    path: String::new(),
                    created: String::new(),
                    starred: None,
                    starred_at: None,
                    explicit_status: None,
                    replay_gain: crate::navidrome::models::ReplayGain::default(),
                }],
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["album"]["id"], "al1");
        assert_eq!(json["album"]["coverArt"], "https://example.com/c.jpg");
        assert_eq!(json["album"]["song"][0]["contentType"], "audio/flac");
        assert_eq!(json["album"]["song"][0]["type"], "song");
    }

    #[test]
    fn album_id3_serializes_coverart_camelcase() {
        let album = AlbumId3 {
            id: "al1".into(),
            album: "A".into(),
            title: "A".into(),
            name: "A".into(),
            artist: "X".into(),
            artist_id: "ar1".into(),
            cover_art: Some("https://example.com/c.jpg".into()),
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
        };
        let json = serde_json::to_value(&album).unwrap();
        assert_eq!(json["coverArt"], "https://example.com/c.jpg");
        assert!(json.get("cover_art").is_none());
    }
}
