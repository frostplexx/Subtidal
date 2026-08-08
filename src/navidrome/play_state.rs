// Playback session state: the saved play queue and per-track bookmarks.
// Single-user server, in memory; both are per-user by construction. All
// functions take the wall clock as a parameter, so tests run without a
// chrono dependency or timing flakiness.
use chrono::{DateTime, SecondsFormat, Utc};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// The saved play queue from savePlayQueue.
#[derive(Clone, Debug)]
pub struct PlayQueue {
    pub track_ids: Vec<u64>,
    pub current: Option<u64>,
    pub position_ms: u64,
    pub username: String,
    pub changed_by: String,
    pub changed_ms: i64,
}

// One bookmark: a position inside a track.
#[derive(Clone, Debug)]
pub struct Bookmark {
    pub track_id: u64,
    pub position_ms: u64,
    pub comment: String,
    pub username: String,
    pub created_ms: i64,
    pub changed_ms: i64,
}

fn queue_slot() -> &'static Mutex<Option<PlayQueue>> {
    static SLOT: OnceLock<Mutex<Option<PlayQueue>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn bookmark_map() -> &'static Mutex<BTreeMap<u64, Bookmark>> {
    static MAP: OnceLock<Mutex<BTreeMap<u64, Bookmark>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(BTreeMap::new()))
}

// Save the queue. An empty id list clears it, per the OpenSubsonic rule
// for savePlayQueue.
pub fn save_queue(state: PlayQueue) {
    *queue_slot().lock().unwrap() = if state.track_ids.is_empty() {
        None
    } else {
        Some(state)
    };
}

// The saved queue, if any.
pub fn queue() -> Option<PlayQueue> {
    queue_slot().lock().unwrap().clone()
}

// Upsert a bookmark; an update keeps the original created time.
pub fn upsert_bookmark(track_id: u64, position_ms: u64, comment: String, username: String, now: i64) {
    let mut map = bookmark_map().lock().unwrap();
    let created_ms = map.get(&track_id).map(|b| b.created_ms).unwrap_or(now);
    map.insert(
        track_id,
        Bookmark {
            track_id,
            position_ms,
            comment,
            username,
            created_ms,
            changed_ms: now,
        },
    );
}

// Remove a bookmark. Returns false when none existed.
pub fn delete_bookmark(track_id: u64) -> bool {
    bookmark_map().lock().unwrap().remove(&track_id).is_some()
}

// All bookmarks, oldest first.
pub fn bookmarks() -> Vec<Bookmark> {
    let mut all: Vec<Bookmark> = bookmark_map().lock().unwrap().values().cloned().collect();
    all.sort_by_key(|b| b.created_ms);
    all
}

// Clear both stores. Tests only: state persists across tests in one
// process, so each test starts from a clean slate.
#[cfg(test)]
pub fn reset() {
    *queue_slot().lock().unwrap() = None;
    bookmark_map().lock().unwrap().clear();
}

// Epoch ms -> "YYYY-MM-DDTHH:MM:SSZ" (UTC). Subsonic timestamps are
// RFC 3339; fractional seconds are optional, so they are omitted.
pub fn iso8601_z(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The stores are process-wide statics; tests mutate them from
    // parallel threads, so each state test takes a lock that serializes
    // them, mirroring the now_playing module's pattern.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn save_then_queue_returns_it() {
        let _g = lock();
        reset();
        // Distinct ids: the playqueue handler tests write the same global
        // queue concurrently, so an interleaved write cannot satisfy these.
        save_queue(PlayQueue {
            track_ids: vec![10_001, 10_002],
            current: Some(10_002),
            position_ms: 5_000,
            username: "admin".into(),
            changed_by: "test".into(),
            changed_ms: 100,
        });
        let q = queue().unwrap();
        assert_eq!(q.track_ids, vec![10_001, 10_002]);
        assert_eq!(q.current, Some(10_002));
        assert_eq!(q.position_ms, 5_000);
    }

    #[test]
    fn empty_id_list_clears_the_queue() {
        let _g = lock();
        reset();
        save_queue(PlayQueue {
            track_ids: vec![],
            current: None,
            position_ms: 0,
            username: "admin".into(),
            changed_by: "test".into(),
            changed_ms: 100,
        });
        assert!(queue().is_none());
    }

    #[test]
    fn latest_save_wins() {
        let _g = lock();
        reset();
        save_queue(PlayQueue {
            track_ids: vec![10_001],
            current: Some(10_001),
            position_ms: 0,
            username: "a".into(),
            changed_by: "c".into(),
            changed_ms: 100,
        });
        save_queue(PlayQueue {
            track_ids: vec![10_003, 10_004],
            current: Some(10_004),
            position_ms: 9_000,
            username: "b".into(),
            changed_by: "d".into(),
            changed_ms: 200,
        });
        let q = queue().unwrap();
        assert_eq!(q.track_ids, vec![10_003, 10_004]);
        assert_eq!(q.username, "b");
    }

    #[test]
    fn upsert_keeps_original_created_time() {
        let _g = lock();
        reset();
        upsert_bookmark(42, 10_000, "chapter one".into(), "admin".into(), 100);
        upsert_bookmark(42, 25_000, "chapter two".into(), "admin".into(), 200);
        let all = bookmarks();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].position_ms, 25_000);
        assert_eq!(all[0].comment, "chapter two");
        assert_eq!(all[0].username, "admin");
        assert_eq!(all[0].created_ms, 100);
        assert_eq!(all[0].changed_ms, 200);
    }

    #[test]
    fn delete_removes_only_the_named_track() {
        let _g = lock();
        reset();
        upsert_bookmark(1, 1_000, "".into(), "admin".into(), 100);
        upsert_bookmark(2, 2_000, "".into(), "admin".into(), 200);
        assert!(delete_bookmark(1));
        assert!(!delete_bookmark(1));
        let all = bookmarks();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].track_id, 2);
    }

    #[test]
    fn bookmarks_are_ordered_oldest_first() {
        let _g = lock();
        reset();
        upsert_bookmark(3, 1_000, "".into(), "admin".into(), 300);
        upsert_bookmark(1, 1_000, "".into(), "admin".into(), 100);
        upsert_bookmark(2, 1_000, "".into(), "admin".into(), 200);
        let ids: Vec<u64> = bookmarks().iter().map(|b| b.track_id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn iso8601_z_formats_epoch_known_values() {
        assert_eq!(iso8601_z(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_z(1_673_776_800_000), "2023-01-15T10:00:00Z");
        assert_eq!(iso8601_z(1_672_617_600_000), "2023-01-02T00:00:00Z");
    }
}
