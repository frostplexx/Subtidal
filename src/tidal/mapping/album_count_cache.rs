// Maps a Tidal artist id to the size of its release list, as getArtist
// would serve it (albums + EPs/singles + compilations, deduplicated).
// Tidal reports no album count on any artist object, so the count only
// exists once an artist's releases have been listed; every artist shape
// (index, search, starred, similar) reads it from here and leaves
// albumCount out until then. The TTL matches meta_cache, which holds
// the release lists the counts came from.
use std::sync::LazyLock;
use std::time::Duration;

use moka::sync::Cache;

static ALBUM_COUNTS: LazyLock<Cache<u64, u32>> = LazyLock::new(|| {
    Cache::builder()
        .time_to_live(Duration::from_secs(6 * 3600))
        .max_capacity(20_000)
        .build()
});

pub(crate) fn remember(artist_id: u64, count: u32) {
    ALBUM_COUNTS.insert(artist_id, count);
}

pub(crate) fn lookup(artist_id: u64) -> Option<u32> {
    ALBUM_COUNTS.get(&artist_id)
}

// See crate::maintenance: moka applies expiry during pending-task
// maintenance, which an idle server never triggers on its own.
pub(crate) fn run_pending_cache_tasks() {
    ALBUM_COUNTS.run_pending_tasks();
}
