// Periodic cache housekeeping.
//
// moka 0.12 dropped the background maintenance threads it had in 0.11:
// expiry and size eviction are now applied inside `run_pending_tasks`,
// which the cache calls opportunistically from its own reads and writes.
// That is enough while traffic flows, but it means a TTL is only a
// *logical* guarantee — an expired entry stops being returned by `get`,
// yet its value stays allocated until some later operation runs the
// pending tasks.
//
// A music server is idle most of the time, and an idle process performs
// no cache operations at all. So everything the last listening session
// left behind (audio bodies, CDN segments, stream manifests, Tidal JSON
// pages) stayed resident for as long as the container ran, long past the
// 2-minute and 5-minute TTLs that were supposed to release it. Restarting
// the container was the only thing that actually freed it.
//
// This janitor supplies the ticks moka no longer has, so the TTLs release
// memory on schedule whether or not anyone is listening.

use std::time::Duration;

// Long enough to be free (a sweep over expired entries is cheap and does
// no IO), short enough that the 120s manifest TTL still means something.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

pub fn spawn() {
    tokio::spawn(async {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        // The first tick fires immediately; skip straight to the cadence.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            sweep().await;
        }
    });
}

async fn sweep() {
    crate::navidrome::handlers::stream::run_pending_cache_tasks();
    crate::navidrome::handlers::lyrics::run_pending_cache_tasks();
    crate::tidal::mapping::cover_cache::run_pending_cache_tasks();
    crate::tidal::mapping::album_count_cache::run_pending_cache_tasks();
    // client_opt rather than client(): the janitor must not panic if it
    // ever ticks before the Tidal client is initialized.
    if let Some(client) = crate::tidal::client_opt() {
        client.run_pending_cache_tasks().await;
    }
}
