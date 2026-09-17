// Playback session state: the saved play queue, per-track bookmarks, and
// the local album play history. Single-user server; all three are
// per-user by construction. Held in memory and mirrored to the state
// file once `init` has loaded it, so they survive a restart. All
// functions take the wall clock as a parameter, so tests run without
// timing flakiness; tests never call init, so they never touch the file.
use crate::navidrome::models::Child;
use crate::state;
use chrono::{DateTime, SecondsFormat};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

// The saved play queue from savePlayQueue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayQueue {
    pub track_ids: Vec<u64>,
    // The current song as an id (plain savePlayQueue semantics) and as
    // an index into track_ids (indexBasedQueue semantics). The index
    // round-trips verbatim to the ByIndex endpoints; feeding clients
    // the id and re-deriving the index shifts when dead tracks are
    // dropped or ids repeat.
    pub current: Option<u64>,
    pub current_index: Option<usize>,
    pub position_ms: u64,
    pub username: String,
    pub changed_by: String,
    pub changed_ms: i64,
}

// The queue as served: the raw id list it was built from, plus the
// resolved song detail and the raw positions of ids Tidal no longer
// serves. Rebuilt when the raw id list changes, so a repeated poll
// never re-fetches Tidal.
#[derive(Clone)]
pub struct ResolvedQueue {
    pub for_ids: Vec<u64>,
    pub entries: Vec<Child>,
    pub dropped_positions: Vec<usize>,
}

// One bookmark: a position inside a track.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bookmark {
    pub track_id: u64,
    pub position_ms: u64,
    pub comment: String,
    pub username: String,
    pub created_ms: i64,
    pub changed_ms: i64,
}

// One album's local play record, fed by completed scrobbles.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayRecord {
    pub album_id: u64,
    pub play_count: u32,
    pub last_played_ms: i64,
}

// Albums tracked in the history before the least recently played drop.
const HISTORY_CAP: usize = 500;

// The on-disk shape: everything but the resolved queue, which is a
// derived cache rebuilt from track_ids on the next read.
#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    #[serde(default)]
    queue: Option<PlayQueue>,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
    #[serde(default)]
    history: Vec<PlayRecord>,
}

static PERSIST: AtomicBool = AtomicBool::new(false);
// Snapshot generation: background writes may complete out of order,
// so each one skips itself when a newer snapshot already landed.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static WRITTEN: Mutex<u64> = Mutex::new(0);

fn queue_slot() -> &'static Mutex<Option<PlayQueue>> {
    static SLOT: OnceLock<Mutex<Option<PlayQueue>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn resolved_slot() -> &'static Mutex<Option<ResolvedQueue>> {
    static SLOT: OnceLock<Mutex<Option<ResolvedQueue>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn bookmark_map() -> &'static Mutex<BTreeMap<u64, Bookmark>> {
    static MAP: OnceLock<Mutex<BTreeMap<u64, Bookmark>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn history_map() -> &'static Mutex<BTreeMap<u64, PlayRecord>> {
    static MAP: OnceLock<Mutex<BTreeMap<u64, PlayRecord>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(BTreeMap::new()))
}

// Load the stored state and turn on mirroring. A missing section is a
// fresh install; an unreadable one is logged and treated as empty
// rather than refusing to start.
pub fn init() {
    match state::load_section::<Persisted>(state::PLAYBACK) {
        Ok(Some(p)) => {
            *queue_slot().lock().unwrap_or_else(|e| e.into_inner()) = p.queue;
            *bookmark_map().lock().unwrap_or_else(|e| e.into_inner()) =
                p.bookmarks.into_iter().map(|b| (b.track_id, b)).collect();
            *history_map().lock().unwrap_or_else(|e| e.into_inner()) =
                p.history.into_iter().map(|r| (r.album_id, r)).collect();
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("playback state not restored: {e}"),
    }
    PERSIST.store(true, Ordering::Relaxed);
}

// Mirror the current state to the file. Callers invoke this after
// releasing their store lock; the snapshot takes each lock briefly.
// The write itself moves off the async runtime; the state file has
// its own lock, so overlapping writes serialize there.
fn persist() {
    if !PERSIST.load(Ordering::Relaxed) {
        return;
    }
    let snapshot = Persisted {
        queue: queue(),
        bookmarks: bookmarks(),
        history: history_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect(),
    };
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let write = move || {
        let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        if *written >= generation {
            return;
        }
        match state::store_section(state::PLAYBACK, &snapshot) {
            Ok(()) => *written = generation,
            Err(e) => tracing::warn!("playback state not saved: {e}"),
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(write);
        }
        Err(_) => write(),
    }
}

// Save the queue. An empty id list clears it, per the OpenSubsonic rule
// for savePlayQueue.
pub fn save_queue(state: PlayQueue) {
    *queue_slot().lock().unwrap_or_else(|e| e.into_inner()) = if state.track_ids.is_empty() {
        None
    } else {
        Some(state)
    };
    persist();
}

// The saved queue, if any.
pub fn queue() -> Option<PlayQueue> {
    queue_slot().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// Replace the resolved queue. A cleared store resolves to None.
pub fn save_resolved(state: Option<ResolvedQueue>) {
    *resolved_slot().lock().unwrap_or_else(|e| e.into_inner()) = state;
}

// The resolved queue, if any.
pub fn resolved() -> Option<ResolvedQueue> {
    resolved_slot().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// Upsert a bookmark; an update keeps the original created time.
pub fn upsert_bookmark(track_id: u64, position_ms: u64, comment: String, username: String, now: i64) {
    let mut map = bookmark_map().lock().unwrap_or_else(|e| e.into_inner());
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
    drop(map);
    persist();
}

// Remove a bookmark. Returns false when none existed.
pub fn delete_bookmark(track_id: u64) -> bool {
    let removed = bookmark_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&track_id)
        .is_some();
    if removed {
        persist();
    }
    removed
}

// Count one completed play of an album. The history stays bounded by
// dropping the least recently played album once it overflows.
pub fn record_play(album_id: u64, now: i64) {
    let mut map = history_map().lock().unwrap_or_else(|e| e.into_inner());
    let rec = map.entry(album_id).or_insert(PlayRecord {
        album_id,
        play_count: 0,
        last_played_ms: now,
    });
    rec.play_count = rec.play_count.saturating_add(1);
    rec.last_played_ms = now;
    if map.len() > HISTORY_CAP
        && let Some(oldest) = map.values().min_by_key(|r| r.last_played_ms).map(|r| r.album_id)
    {
        map.remove(&oldest);
    }
    drop(map);
    persist();
}

// Count a play from a Tidal track JSON, when it names its album.
pub fn record_play_from_track(track: &serde_json::Value, now: i64) {
    if let Some(album_id) = track["album"]["id"].as_u64() {
        record_play(album_id, now);
    }
}

// Album play records, most recently played first.
pub fn recent_albums() -> Vec<PlayRecord> {
    let mut all: Vec<PlayRecord> = history_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();
    all.sort_by_key(|r| std::cmp::Reverse(r.last_played_ms));
    all
}

// Album play records, most played first; ties break on recency.
pub fn frequent_albums() -> Vec<PlayRecord> {
    let mut all = recent_albums();
    all.sort_by_key(|r| std::cmp::Reverse(r.play_count));
    all
}

// The play count of one album, 0 when never played.
pub fn play_count(album_id: u64) -> u32 {
    history_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&album_id)
        .map(|r| r.play_count)
        .unwrap_or(0)
}

// All bookmarks, oldest first.
pub fn bookmarks() -> Vec<Bookmark> {
    let mut all: Vec<Bookmark> = bookmark_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();
    all.sort_by_key(|b| b.created_ms);
    all
}

// Clear both stores. Tests only: state persists across tests in one
// process, so each test starts from a clean slate.
#[cfg(test)]
pub fn reset() {
    *queue_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    *resolved_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    bookmark_map().lock().unwrap_or_else(|e| e.into_inner()).clear();
    history_map().lock().unwrap_or_else(|e| e.into_inner()).clear();
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
            current_index: Some(1),
            position_ms: 5_000,
            username: "admin".into(),
            changed_by: "test".into(),
            changed_ms: 100,
        });
        let q = queue().unwrap();
        assert_eq!(q.track_ids, vec![10_001, 10_002]);
        assert_eq!(q.current, Some(10_002));
        assert_eq!(q.current_index, Some(1));
        assert_eq!(q.position_ms, 5_000);
    }

    #[test]
    fn empty_id_list_clears_the_queue() {
        let _g = lock();
        reset();
        save_queue(PlayQueue {
            track_ids: vec![],
            current: None,
            current_index: None,
            position_ms: 0,
            username: "admin".into(),
            changed_by: "test".into(),
            changed_ms: 100,
        });
        assert!(queue().is_none());    }

    #[test]
    fn latest_save_wins() {
        let _g = lock();
        reset();
        save_queue(PlayQueue {
            track_ids: vec![10_001],
            current: Some(10_001),
            current_index: Some(0),
            position_ms: 0,
            username: "a".into(),
            changed_by: "c".into(),
            changed_ms: 100,
        });
        save_queue(PlayQueue {
            track_ids: vec![10_003, 10_004],
            current: Some(10_004),
            current_index: Some(1),
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
    fn history_orders_by_recency_and_frequency() {
        let _g = lock();
        reset();
        record_play(1, 100);
        record_play(2, 200);
        record_play(1, 300);
        record_play(3, 250);
        let recent: Vec<u64> = recent_albums().iter().map(|r| r.album_id).collect();
        assert_eq!(recent, vec![1, 3, 2]);
        let frequent: Vec<u64> = frequent_albums().iter().map(|r| r.album_id).collect();
        // Album 1 played twice; the rest tie and keep recency order.
        assert_eq!(frequent, vec![1, 3, 2]);
        assert_eq!(play_count(1), 2);
        assert_eq!(play_count(9), 0);
        record_play_from_track(&serde_json::json!({"album": {"id": 4}}), 400);
        assert_eq!(play_count(4), 1);
    }

    #[test]
    fn history_drops_the_least_recent_past_the_cap() {
        let _g = lock();
        reset();
        for i in 0..=HISTORY_CAP as u64 {
            record_play(i, i as i64);
        }
        assert_eq!(recent_albums().len(), HISTORY_CAP);
        assert_eq!(play_count(0), 0, "album 0 was the least recently played");
        assert_eq!(play_count(HISTORY_CAP as u64), 1);
    }

    #[test]
    fn persisted_roundtrips_through_json() {
        let p = Persisted {
            queue: Some(PlayQueue {
                track_ids: vec![1, 2],
                current: Some(2),
                current_index: Some(1),
                position_ms: 5,
                username: "u".into(),
                changed_by: "c".into(),
                changed_ms: 9,
            }),
            bookmarks: vec![Bookmark {
                track_id: 7,
                position_ms: 1,
                comment: "x".into(),
                username: "u".into(),
                created_ms: 1,
                changed_ms: 2,
            }],
            history: vec![PlayRecord { album_id: 3, play_count: 2, last_played_ms: 4 }],
        };
        let text = serde_json::to_string(&p).unwrap();
        let back: Persisted = serde_json::from_str(&text).unwrap();
        assert_eq!(back.queue.unwrap().track_ids, vec![1, 2]);
        assert_eq!(back.bookmarks[0].track_id, 7);
        assert_eq!(back.history[0].play_count, 2);
        // Older files without the section fields still parse.
        let empty: Persisted = serde_json::from_str("{}").unwrap();
        assert!(empty.queue.is_none());
    }

    #[test]
    fn iso8601_z_formats_epoch_known_values() {
        assert_eq!(iso8601_z(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_z(1_673_776_800_000), "2023-01-15T10:00:00Z");
        assert_eq!(iso8601_z(1_672_617_600_000), "2023-01-02T00:00:00Z");
    }
}
