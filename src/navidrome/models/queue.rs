// Play queue and bookmark models.
use serde::Serialize;

use super::song::Child;

// getPlayQueue data: { playQueue: { current, position, username,
// changed, changedBy, entry } }
#[derive(Serialize)]
pub struct PlayQueueResponse {
    #[serde(rename = "playQueue")]
    pub play_queue: PlayQueue,
}

#[derive(Serialize)]
pub struct PlayQueue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    pub position: u64,
    pub username: String,
    pub changed: String,
    #[serde(rename = "changedBy")]
    pub changed_by: String,
    pub entry: Vec<Child>,
}

// getPlayQueueByIndex data (OpenSubsonic indexBasedQueue): like
// playQueue, but the current song is the queue index, not the song id.
#[derive(Serialize)]
pub struct PlayQueueByIndexResponse {
    #[serde(rename = "playQueueByIndex")]
    pub play_queue: PlayQueueByIndex,
}

#[derive(Serialize)]
pub struct PlayQueueByIndex {
    #[serde(rename = "currentIndex", skip_serializing_if = "Option::is_none")]
    pub current_index: Option<u32>,
    pub position: u64,
    pub username: String,
    pub changed: String,
    #[serde(rename = "changedBy")]
    pub changed_by: String,
    pub entry: Vec<Child>,
}

// getBookmarks data: { bookmarks: { bookmark: [ Bookmark ] } }
#[derive(Serialize)]
pub struct BookmarksResponse {
    pub bookmarks: Bookmarks,
}

#[derive(Serialize)]
pub struct Bookmarks {
    pub bookmark: Vec<Bookmark>,
}

#[derive(Serialize)]
pub struct Bookmark {
    pub entry: Child,
    pub position: u64,
    pub username: String,
    pub comment: String,
    pub created: String,
    pub changed: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: u64) -> Child {
        Child {
            id: format!("t{id}"),
            parent: "al1".into(),
            is_dir: false,
            is_video: false,
            title: "title".into(),
            album: "album".into(),
            artist: "artist".into(),
            track: 1,
            year: Some(2023),
            genre: Some("rock".into()),
            genres: None,
            cover_art: None,
            duration: 200,
            disc_number: None,
            album_id: "al1".into(),
            artist_id: "ar1".into(),
            kind: "song",
            content_type: "audio/flac",
            suffix: "flac",
            size: 1234,
            path: "/track.flac".into(),
            created: "2023-01-01T00:00:00Z".into(),
            starred: None,
            starred_at: None,
            replay_gain: crate::navidrome::models::ReplayGain::default(),
        }
    }

    #[test]
    fn play_queue_serializes_subsonic_fields() {
        let pq = PlayQueue {
            current: Some("t2".into()),
            position: 5_000,
            username: "demo".into(),
            changed: "2023-03-10T02:19:35Z".into(),
            changed_by: "client".into(),
            entry: vec![song(2)],
        };
        let v = serde_json::to_value(PlayQueueResponse { play_queue: pq }).unwrap();
        let pq = &v["playQueue"];
        assert_eq!(pq["current"], "t2");
        assert_eq!(pq["position"], 5_000);
        assert_eq!(pq["username"], "demo");
        assert_eq!(pq["changedBy"], "client");
        assert_eq!(pq["entry"][0]["id"], "t2");
    }

    #[test]
    fn play_queue_omits_current_when_absent() {
        let pq = PlayQueue {
            current: None,
            position: 0,
            username: "demo".into(),
            changed: "2023-03-10T02:19:35Z".into(),
            changed_by: "client".into(),
            entry: vec![],
        };
        let v = serde_json::to_value(PlayQueueResponse { play_queue: pq }).unwrap();
        assert!(v["playQueue"].get("current").is_none());
        assert!(v["playQueue"]["entry"].as_array().unwrap().is_empty());
    }

    #[test]
    fn play_queue_by_index_serializes_os_fields() {
        let pq = PlayQueueByIndex {
            current_index: Some(1),
            position: 5_000,
            username: "demo".into(),
            changed: "2023-03-10T02:19:35Z".into(),
            changed_by: "client".into(),
            entry: vec![song(2)],
        };
        let v = serde_json::to_value(PlayQueueByIndexResponse { play_queue: pq }).unwrap();
        let pq = &v["playQueueByIndex"];
        assert_eq!(pq["currentIndex"], 1);
        assert_eq!(pq["position"], 5_000);
        assert_eq!(pq["changedBy"], "client");
        assert_eq!(pq["entry"][0]["id"], "t2");
    }

    #[test]
    fn play_queue_by_index_omits_current_index_when_absent() {
        let pq = PlayQueueByIndex {
            current_index: None,
            position: 0,
            username: "demo".into(),
            changed: "2023-03-10T02:19:35Z".into(),
            changed_by: "client".into(),
            entry: vec![],
        };
        let v = serde_json::to_value(PlayQueueByIndexResponse { play_queue: pq }).unwrap();
        assert!(v["playQueueByIndex"].get("currentIndex").is_none());
    }

    #[test]
    fn bookmark_serializes_subsonic_fields() {
        let bm = Bookmark {
            entry: song(9),
            position: 129_000,
            username: "demo".into(),
            comment: "chapter".into(),
            created: "2023-03-13T16:30:35Z".into(),
            changed: "2023-03-13T16:30:35Z".into(),
        };
        let v = serde_json::to_value(BookmarksResponse {
            bookmarks: Bookmarks { bookmark: vec![bm] },
        })
        .unwrap();
        let bm = &v["bookmarks"]["bookmark"][0];
        assert_eq!(bm["entry"]["id"], "t9");
        assert_eq!(bm["position"], 129_000);
        assert_eq!(bm["comment"], "chapter");
        assert_eq!(bm["changed"], "2023-03-13T16:30:35Z");
    }
}
