// Playlist models.
use serde::Serialize;

use super::song::Child;

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

// getPlaylist data: { playlist: { ...Playlist fields, entry: [ Child ] } }
#[derive(Serialize)]
pub struct GetPlaylistResponse {
    pub playlist: PlaylistWithSongs,
}

#[derive(Serialize)]
pub struct PlaylistWithSongs {
    #[serde(flatten)]
    pub playlist: Playlist,
    pub entry: Vec<Child>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_with_songs_flattens_header_and_entry() {
        let p = PlaylistWithSongs {
            playlist: Playlist {
                id: "abc-123".into(),
                name: "Morning".into(),
                comment: None,
                owner: Some("Ada".into()),
                r#public: true,
                song_count: Some(1),
                duration: Some(220),
                created: None,
                changed: None,
                cover_art: None,
            },
            entry: vec![Child {
                id: "t1".into(),
                parent: "al2".into(),
                is_dir: false,
                is_video: false,
                title: "Song One".into(),
                album: "Album One".into(),
                artist: "Artist A".into(),
                track: 1,
                year: None,
                genre: None,
                cover_art: None,
                duration: 220,
                disc_number: None,
                album_id: "al2".into(),
                artist_id: "ar3".into(),
                kind: "song",
                content_type: "audio/flac",
                suffix: "flac",
                size: 0,
                path: String::new(),
                created: String::new(),
                starred: None,
                replay_gain: crate::navidrome::models::ReplayGain::default(),
            }],
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.starts_with(r##"{"id":"abc-123""##));
        assert!(json.contains(r##"title":"Song One""##));
        assert!(json.ends_with(r##"}"##));
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
}
