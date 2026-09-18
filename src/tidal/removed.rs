// Tracks Tidal has refused at every quality tier (Error::TrackRemoved):
// pulled from the catalogue by the rights holder. Album and playlist
// listings still carry them, and the metadata cache keeps serving those
// listings for a while, so a track found dead at stream time is
// remembered here and marked unavailable on the next listing rather than
// waiting for the cache to expire. Process-local; a restart forgets it,
// and the listing flags (allowStreaming/streamReady) take over once the
// cache refreshes.
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

fn set() -> &'static Mutex<HashSet<u64>> {
    static SET: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn mark(track_id: u64) {
    set().lock().unwrap().insert(track_id);
}

pub fn contains(track_id: u64) -> bool {
    set().lock().unwrap().contains(&track_id)
}
