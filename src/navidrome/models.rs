use serde::Serialize;

// Root of every response: { "subsonic-response": { ... } }
#[derive(Serialize)]
pub struct SubsonicResponse<T: Serialize> {
    #[serde(rename = "subsonic-response")]
    pub inner: T,
}

// Common fields inside subsonic-response, plus per-endpoint data
#[derive(Serialize)]
pub struct SubsonicBody<T: Serialize> {
    pub status: &'static str,
    pub version: &'static str, // supported Subsonic API version
    #[serde(rename = "type")] // "type" is a Rust keyword
    pub server_type: &'static str,
    #[serde(rename = "serverVersion")]
    pub server_version: &'static str,
    #[serde(rename = "openSubsonic")]
    pub open_subsonic: bool,
    #[serde(flatten)]
    pub data: T,
}

// ping returns no extra data
#[derive(Serialize)]
pub struct PingResponse {}

// getOpenSubsonicExtensions data: { openSubsonicExtensions: [ { name, versions } ] }
#[derive(Serialize)]
pub struct GetOpenSubsonicExtensionsResponse {
    #[serde(rename = "openSubsonicExtensions")]
    pub extensions: Vec<OpenSubsonicExtension>,
}

#[derive(Serialize)]
pub struct OpenSubsonicExtension {
    pub name: &'static str,
    pub versions: Vec<u32>,
}

// Error body: { status: "failed", ..., error: { code, message } }
#[derive(Serialize)]
pub struct SubsonicErrorBody {
    pub status: &'static str,
    pub version: &'static str,
    #[serde(rename = "type")]
    pub server_type: &'static str,
    #[serde(rename = "serverVersion")]
    pub server_version: &'static str,
    #[serde(rename = "openSubsonic")]
    pub open_subsonic: bool,
    pub error: SubsonicError,
}

#[derive(Serialize)]
pub struct SubsonicError {
    pub code: u32,
    pub message: &'static str,
}

// getUser data: { user: { ... } }
#[derive(Serialize)]
pub struct GetUserResponse {
    pub user: User,
}

// Role flags are strings ("true"/"false") to match the documented
// OpenSubsonic JSON output, a legacy Subsonic quirk.
#[derive(Serialize)]
pub struct User {
    pub folder: Vec<u32>,
    pub username: String,
    pub email: String,
    #[serde(rename = "scrobblingEnabled")]
    pub scrobbling_enabled: &'static str,
    #[serde(rename = "adminRole")]
    pub admin_role: &'static str,
    #[serde(rename = "settingsRole")]
    pub settings_role: &'static str,
    #[serde(rename = "downloadRole")]
    pub download_role: &'static str,
    #[serde(rename = "uploadRole")]
    pub upload_role: &'static str,
    #[serde(rename = "playlistRole")]
    pub playlist_role: &'static str,
    #[serde(rename = "coverArtRole")]
    pub cover_art_role: &'static str,
    #[serde(rename = "commentRole")]
    pub comment_role: &'static str,
    #[serde(rename = "podcastRole")]
    pub podcast_role: &'static str,
    #[serde(rename = "streamRole")]
    pub stream_role: &'static str,
    #[serde(rename = "jukeboxRole")]
    pub jukebox_role: &'static str,
    #[serde(rename = "shareRole")]
    pub share_role: &'static str,
}

// search3 data: { searchResult3: { artist, album, song } }
#[derive(Serialize)]
pub struct SearchResult3Response {
    #[serde(rename = "searchResult3")]
    pub search_result: SearchResult3,
}

#[derive(Serialize)]
pub struct SearchResult3 {
    pub artist: Vec<ArtistId3>,
    pub album: Vec<AlbumId3>,
    pub song: Vec<Child>,
}

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

// getPlaylists data: { playlists: { playlist: [ Playlist ] } }
#[derive(Serialize)]
pub struct PlaylistsResponse {
    pub playlists: Playlists,
}

#[derive(Serialize)]
pub struct Playlists {
    pub playlist: Vec<Playlist>,
}

// Subsonic playlist. Tidal ids are UUIDs; Subsonic keeps them opaque.
#[derive(Serialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(rename = "public")]
    pub r#public: bool,
    #[serde(rename = "songCount", skip_serializing_if = "Option::is_none")]
    pub song_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<String>,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
}

// getGenres data: { genres: { genre: [ Genre ] } }
#[derive(Serialize)]
pub struct GenresResponse {
    pub genres: Genres,
}

#[derive(Serialize)]
pub struct Genres {
    pub genre: Vec<Genre>,
}

#[derive(Serialize)]
pub struct Genre {
    pub name: String,
    #[serde(rename = "songCount", skip_serializing_if = "Option::is_none")]
    pub song_count: Option<u32>,
    #[serde(rename = "albumCount", skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u32>,
}

// jukeboxControl data: jukeboxStatus (all actions); get also returns
// jukeboxPlaylist.
#[derive(Serialize)]
pub struct JukeboxControlResponse {
    #[serde(rename = "jukeboxStatus")]
    pub status: JukeboxStatus,
    #[serde(rename = "jukeboxPlaylist", skip_serializing_if = "Option::is_none")]
    pub playlist: Option<JukeboxPlaylist>,
}

#[derive(Serialize)]
pub struct JukeboxStatus {
    #[serde(rename = "currentIndex")]
    pub current_index: u32,
    pub playing: bool,
    pub gain: f32,
    pub position: u32,
}

#[derive(Serialize)]
pub struct JukeboxPlaylist {
    pub entry: Vec<Child>,
}

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
}

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

#[derive(Serialize)]
pub struct ArtistId3 {
    pub id: String,
    pub name: String,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(rename = "albumCount", skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
                }],
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["artist"]["id"], "ar1");
        assert_eq!(json["artist"]["albumCount"], 2);
        assert_eq!(json["artist"]["album"][0]["id"], "al1");
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
        };
        let json = serde_json::to_value(&album).unwrap();
        assert_eq!(json["coverArt"], "https://example.com/c.jpg");
        assert!(json.get("cover_art").is_none());
    }

    #[test]
    fn ping_response_has_subsonic_response_root_key() {
        let resp = SubsonicResponse {
            inner: SubsonicBody {
                status: "ok",
                version: "1.16.1",
                server_type: "Subtidal",
                server_version: "0.1.0",
                open_subsonic: true,
                data: PingResponse {},
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.starts_with(r##"{"subsonic-response":"##));
    }

    #[test]
    fn open_subsonic_extensions_use_versions_array() {
        let resp = GetOpenSubsonicExtensionsResponse {
            extensions: vec![OpenSubsonicExtension {
                name: "transcodeOffset",
                versions: vec![1],
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r##"{"openSubsonicExtensions":[{"name":"transcodeOffset","versions":[1]}]}"##
        );
    }

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
                }],
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r##"{"albumList2":{"album":[{"id":"al1","album":"A","title":"A","name":"A","artist":"X","artistId":"ar1","songCount":20,"duration":4248,"playCount":0,"created":"2021-07-22T02:09:31+00:00","year":2005,"genre":"Hip-Hop"}]}}"##
        );
    }

    #[test]
    fn playlist_serializes_subsonic_fields() {
        let p = Playlist {
            id: "abc-123".into(),
            name: "Morning".into(),
            comment: None,
            owner: Some("Ada".into()),
            r#public: true,
            song_count: Some(42),
            duration: Some(9134),
            created: None,
            changed: None,
            cover_art: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(
            json,
            r##"{"id":"abc-123","name":"Morning","owner":"Ada","public":true,"songCount":42,"duration":9134}"##
        );
    }

    #[test]
    fn jukebox_control_serializes_status_and_playlist() {
        let resp = JukeboxControlResponse {
            status: JukeboxStatus {
                current_index: 7,
                playing: true,
                gain: 0.9,
                position: 67,
            },
            playlist: Some(JukeboxPlaylist { entry: vec![] }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r##"{"jukeboxStatus":{"currentIndex":7,"playing":true,"gain":0.9,"position":67},"jukeboxPlaylist":{"entry":[]}}"##
        );
    }
}
