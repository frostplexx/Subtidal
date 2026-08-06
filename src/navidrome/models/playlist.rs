// Playlist models.
use serde::Serialize;

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

#[cfg(test)]
mod tests {
    use super::*;

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
