// Playback state shared by reportPlayback/updateNowPlaying/scrobble and
// getNowPlaying. Single-user server: one slot, replaced by each report,
// expired after ten minutes without one. Positions are estimated from
// the last report while the state is playing.
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// Expire an entry after ten minutes without a report; a paused track
// then stops showing as playing.
const STALE_MS: i64 = 10 * 60 * 1000;

#[derive(Clone, Debug)]
pub struct NowPlaying {
    pub track_id: u64,
    pub username: String,
    // starting | playing | paused (stopped clears the slot)
    pub state: &'static str,
    pub position_ms: u64,
    pub playback_rate: f64,
    // Wall clock of the last report; the anchor for position estimation.
    pub last_report_ms: i64,
    // Wall clock of the playing session start; drives minutesAgo.
    pub started_ms: i64,
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn state() -> &'static Mutex<Option<NowPlaying>> {
    static STATE: OnceLock<Mutex<Option<NowPlaying>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

// Legacy report (updateNowPlaying, scrobble submission=false): the song
// is playing from position 0 at normal speed.
pub fn report(track_id: u64, username: String) {
    report_at(track_id, username, now_ms());
}

pub fn report_at(track_id: u64, username: String, now: i64) {
    *state().lock().unwrap() = Some(NowPlaying {
        track_id,
        username,
        state: "playing",
        position_ms: 0,
        playback_rate: 1.0,
        last_report_ms: now,
        started_ms: now,
    });
}

// reportPlayback: apply a playback timeline event. stopped clears the
// slot; starting restarts the session clock; playing and paused record
// the position anchor.
pub fn report_playback(
    track_id: u64,
    username: String,
    play_state: &'static str,
    position_ms: u64,
    playback_rate: f64,
) {
    report_playback_at(track_id, username, play_state, position_ms, playback_rate, now_ms());
}

pub fn report_playback_at(
    track_id: u64,
    username: String,
    play_state: &'static str,
    position_ms: u64,
    playback_rate: f64,
    now: i64,
) {
    if play_state == "stopped" {
        *state().lock().unwrap() = None;
        return;
    }
    let mut slot = state().lock().unwrap();
    let restart = play_state == "starting"
        || slot.as_ref().is_none_or(|n| n.track_id != track_id);
    let started_ms = if restart {
        now
    } else {
        slot.as_ref().expect("restart implies a slot").started_ms
    };
    *slot = Some(NowPlaying {
        track_id,
        username,
        state: play_state,
        position_ms,
        playback_rate,
        last_report_ms: now,
        started_ms,
    });
}

// The current playback, if a report arrived within the staleness window.
pub fn current() -> Option<NowPlaying> {
    current_at(now_ms())
}

pub fn current_at(now: i64) -> Option<NowPlaying> {
    let slot = state().lock().unwrap();
    match slot.as_ref() {
        Some(n) if now - n.last_report_ms <= STALE_MS => Some(n.clone()),
        _ => None,
    }
}

// Estimated current position: advance from the last report while playing.
pub fn position_at(n: &NowPlaying, now: i64) -> u64 {
    if n.state == "playing" {
        let elapsed = ((now - n.last_report_ms).max(0) as f64 * n.playback_rate) as u64;
        n.position_ms + elapsed
    } else {
        n.position_ms
    }
}

// Serializes tests that mutate the process-wide slot, from this module
// and from handler tests (annotate.rs scrobble gating). A panicked test
// poisons the mutex; recover instead of cascading the failure into
// every other state test.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The slot is a process-wide static; tests mutate it from parallel
    // threads, so each state test takes a lock that serializes them.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        test_lock()
    }

    #[test]
    fn report_then_current_returns_entry() {
        let _g = lock();
        report_at(42, "admin".into(), 1_000_000);
        assert!(current_at(1_000_000 + 60_000).is_some());
    }

    #[test]
    fn stale_report_expires() {
        let _g = lock();
        report_at(42, "admin".into(), 1_000_000);
        assert!(current_at(1_000_000 + STALE_MS + 1).is_none());
    }

    #[test]
    fn latest_report_wins() {
        let _g = lock();
        report_at(1, "a".into(), 100);
        report_at(2, "b".into(), 200);
        let n = current_at(300).unwrap();
        assert_eq!(n.track_id, 2);
        assert_eq!(n.username, "b");
    }

    #[test]
    fn stopped_clears_the_slot() {
        let _g = lock();
        report_at(42, "admin".into(), 1_000_000);
        report_playback_at(42, "admin".into(), "stopped", 220_000, 1.0, 1_100_000);
        assert!(current_at(1_100_000).is_none());
    }

    #[test]
    fn starting_restarts_the_session_clock() {
        let _g = lock();
        report_playback_at(42, "admin".into(), "starting", 0, 1.0, 1_000_000);
        report_playback_at(42, "admin".into(), "playing", 10_000, 2.0, 1_100_000);
        let n = current_at(1_100_000).unwrap();
        assert_eq!(n.state, "playing");
        // started_ms came from the starting event, not the playing one.
        assert_eq!(n.started_ms, 1_000_000);
        assert_eq!(n.playback_rate, 2.0);
    }

    #[test]
    fn new_track_resets_the_session() {
        let _g = lock();
        report_playback_at(1, "admin".into(), "starting", 0, 1.0, 1_000_000);
        report_playback_at(2, "admin".into(), "playing", 5_000, 1.0, 1_200_000);
        let n = current_at(1_200_000).unwrap();
        assert_eq!(n.track_id, 2);
        assert_eq!(n.started_ms, 1_200_000);
    }

    #[test]
    fn position_advances_only_while_playing() {
        let _g = lock();
        report_playback_at(42, "admin".into(), "starting", 0, 1.0, 1_000_000);
        report_playback_at(42, "admin".into(), "playing", 10_000, 1.0, 1_100_000);
        let n = current_at(1_100_000).unwrap();
        assert_eq!(position_at(&n, 1_120_000), 30_000);
        report_playback_at(42, "admin".into(), "paused", 30_000, 1.0, 1_120_000);
        let n = current_at(1_120_000).unwrap();
        assert_eq!(position_at(&n, 1_200_000), 30_000);
    }
}
