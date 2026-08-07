// Now-playing state shared by updateNowPlaying/scrobble reports and
// getNowPlaying. Single-user server: one slot, replaced by each report,
// expired after ten minutes without one.
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// Expire an entry after ten minutes of silence; a paused track then
// stops showing as playing.
const STALE_MS: i64 = 10 * 60 * 1000;

#[derive(Clone, Debug)]
pub struct NowPlaying {
    pub track_id: u64,
    pub username: String,
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

// Record a playback report. The latest report wins.
pub fn report(track_id: u64, username: String) {
    report_at(track_id, username, now_ms());
}

pub fn report_at(track_id: u64, username: String, started_ms: i64) {
    *state().lock().unwrap() = Some(NowPlaying {
        track_id,
        username,
        started_ms,
    });
}

// The current playback, if a report arrived within the staleness window.
pub fn current() -> Option<NowPlaying> {
    current_at(now_ms())
}

pub fn current_at(now_ms: i64) -> Option<NowPlaying> {
    let slot = state().lock().unwrap();
    match slot.as_ref() {
        Some(n) if now_ms - n.started_ms <= STALE_MS => Some(n.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_then_current_returns_entry() {
        report_at(42, "admin".into(), 1_000_000);
        assert!(current_at(1_000_000 + 60_000).is_some());
    }

    #[test]
    fn stale_report_expires() {
        report_at(42, "admin".into(), 1_000_000);
        assert!(current_at(1_000_000 + STALE_MS + 1).is_none());
    }

    #[test]
    fn latest_report_wins() {
        report_at(1, "a".into(), 100);
        report_at(2, "b".into(), 200);
        let n = current_at(300).unwrap();
        assert_eq!(n.track_id, 2);
        assert_eq!(n.username, "b");
    }
}
