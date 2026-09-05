// Play queue: savePlayQueue saves the queue, getPlayQueue returns it
// with fresh song detail. The store is in memory (play_state), so a
// restart clears it. Song ids are fetched one at a time; Tidal offers no
// batch track endpoint. The ByIndex pair (OpenSubsonic indexBasedQueue)
// shares the store and differs only in the wire shape: the current song
// is a queue index (currentIndex) instead of a song id.
use crate::navidrome::ids;
use crate::navidrome::models::{
    Child, PingResponse, PlayQueue, PlayQueueByIndex, PlayQueueByIndexResponse, PlayQueueResponse,
};
use crate::navidrome::now_playing::now_ms;
use crate::navidrome::params::QueryParams;
use crate::navidrome::play_state::{self, ResolvedQueue};
use crate::tidal::client::TidalClient;
use crate::tidal::mapping::song_from_track;
use futures_util::{stream, StreamExt};
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;
use super::playlist::parse_song_ids;
use super::{fail, ok};

// savePlayQueue: replace the saved queue. An empty id list clears it,
// per the OpenSubsonic rule. current is required unless the list is
// empty; position defaults to 0.
pub async fn save_play_queue(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let track_ids = match parse_song_ids(&q.id) {
        Ok(v) => v,
        Err(msg) => return Ok(fail(70, msg)),
    };
    if track_ids.is_empty() {
        play_state::save_queue(play_state::PlayQueue {
            track_ids: vec![],
            current: None,
            position_ms: 0,
            username: q.u.clone().unwrap_or_default(),
            changed_by: q.c.clone().unwrap_or_default(),
            changed_ms: now_ms(),
        });
        // sync_saved_queue(None).await;
        play_state::save_resolved(None);
        return Ok(ok(PingResponse {}));
    }
    let current = match q.current.as_deref() {
        None => return Ok(fail(10, "Required parameter missing")),
        Some(s) => match ids::parse_track_id(s) {
            Some(id) => Some(id),
            None => return Ok(fail(70, "Song not found")),
        },
    };
    play_state::save_queue(play_state::PlayQueue {
        track_ids,
        current,
        position_ms: q.position.unwrap_or(0),
        username: q.u.clone().unwrap_or_default(),
        changed_by: q.c.clone().unwrap_or_default(),
        changed_ms: now_ms(),
    });
    if let Some(client) = crate::tidal::client_opt() {
        tokio::spawn(async move {
            resolve_queue(client).await;
        });
    }
    tracing::info!("savePlayQueue {} songs", q.id.0.len());
    // if let Some(cur) = current {
    //     sync_saved_queue(Some((q.id.clone(), cur))).await;
    // }
    Ok(ok(PingResponse {}))
}

// savePlayQueueByIndex: same as savePlayQueue, but the playing song is
// given by its queue index (currentIndex), not its id. An index outside
// the queue means no current song. currentIndex is optional; Feishin
// sends it only for a non-empty queue.
pub async fn save_play_queue_by_index(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let track_ids = match parse_song_ids(&q.id) {
        Ok(v) => v,
        Err(msg) => return Ok(fail(70, msg)),
    };
    if track_ids.is_empty() {
        play_state::save_queue(play_state::PlayQueue {
            track_ids: vec![],
            current: None,
            position_ms: 0,
            username: q.u.clone().unwrap_or_default(),
            changed_by: q.c.clone().unwrap_or_default(),
            changed_ms: now_ms(),
        });
        // sync_saved_queue(None).await;
        play_state::save_resolved(None);
        return Ok(ok(PingResponse {}));
    }
    let current = q
        .current_index
        .filter(|i| *i >= 0)
        .and_then(|i| track_ids.get(i as usize).copied());
    play_state::save_queue(play_state::PlayQueue {
        track_ids,
        current,
        position_ms: q.position.unwrap_or(0),
        username: q.u.clone().unwrap_or_default(),
        changed_by: q.c.clone().unwrap_or_default(),
        changed_ms: now_ms(),
    });
    if let Some(client) = crate::tidal::client_opt() {
        tokio::spawn(async move {
            resolve_queue(client).await;
        });
    }
    tracing::info!("savePlayQueueByIndex {} songs", q.id.0.len());
    // if let Some(cur) = current {
    //     sync_saved_queue(Some((q.id.clone(), cur))).await;
    // }
    Ok(ok(PingResponse {}))
}

// getPlayQueueByIndex: the saved queue, like getPlayQueue, but the
// current song is reported as its index in the entry list.
pub async fn get_play_queue_by_index(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let client = crate::tidal::client();
    let saved = restore_queue(client).await;
    let entry = resolve_queue(client).await;
    let current_index = saved
        .current
        .and_then(|id| entry.iter().position(|e| e.id == format!("t{id}")))
        .map(|i| i as u32);
    Ok(ok(PlayQueueByIndexResponse {
        play_queue: PlayQueueByIndex {
            current_index,
            position: saved.position_ms,
            username: saved.username,
            changed: play_state::iso8601_z(saved.changed_ms),
            changed_by: saved.changed_by,
            entry,
        },
    }))
}

// Serialize queue resolutions: concurrent polls and the background
// hydration from a save share one Tidal fetch instead of duplicating it.
fn render_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

// The entries to serve: the cached resolution when the raw id list was
// already resolved, otherwise every track fetched in parallel. Tracks
// that Tidal no longer serves are left out of the entries; the raw
// queue is not changed. With the 6h meta_cache warm, most ids resolve
// from memory; the first resolution after a restart is one bounded
// parallel pass, not one per poll.
async fn resolve_queue(client: &'static TidalClient) -> Vec<Child> {
    let Some(q) = play_state::queue() else {
        return Vec::new();
    };
    let _guard = render_lock().lock().await;
    if let Some(r) = play_state::resolved() {
        if r.for_ids == q.track_ids {
            return r.entries;
        }
    }
    let ids = q.track_ids.clone();
    let fetched: Vec<_> = stream::iter(ids.iter().copied())
        .map(|id| async move { (id, client.track(id).await) })
        .buffered(6)
        .collect()
        .await;
    let mut entries = Vec::new();
    for (id, result) in fetched {
        match result {
            Ok(v) => {
                if let Some(song) = song_from_track(&v.to_json()) {
                    entries.push(song);
                }
            }
            Err(e) => {
                tracing::debug!("tidal track {id} fetch failed (dropped from queue): {e}");
            }
        }
    }
    play_state::save_resolved(Some(ResolvedQueue {
        for_ids: ids,
        entries: entries.clone(),
    }));
    entries
}

// getPlayQueue: the saved queue, or an empty one when nothing is saved.
pub async fn get_play_queue(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let client = crate::tidal::client();
    let saved = restore_queue(client).await;
    let entry = resolve_queue(client).await;
    Ok(ok(PlayQueueResponse {
        play_queue: PlayQueue {
            current: saved.current.map(|id| format!("t{id}")),
            position: saved.position_ms,
            username: saved.username,
            changed: play_state::iso8601_z(saved.changed_ms),
            changed_by: saved.changed_by,
            entry,
        },
    }))
}

// The queue to serve: the local store, shared by all Subsonic clients.
// The Tidal mirror used to restore the queue here; it is disabled, so
// a server restart clears the queue (see playqueues.rs).
async fn restore_queue(_client: &TidalClient) -> play_state::PlayQueue {
    if let Some(q) = play_state::queue().filter(|q| !q.track_ids.is_empty()) {
        return q;
    }
    play_state::PlayQueue {
        track_ids: Vec::new(),
        current: None,
        position_ms: 0,
        username: String::new(),
        changed_by: String::new(),
        changed_ms: now_ms(),
    }
}

// Mirror the saved queue to Tidal. Disabled: the feature is gated
// behind an experiment flag that cannot be enabled on the API side.
// #[cfg(not(test))]
// async fn sync_saved_queue(ids: Option<(IdList, u64)>) {
//     let client = crate::tidal::client();
//     let result = match ids {
//         Some((ids, current)) => {
//             let parsed = parse_song_ids(&ids);
//             match parsed {
//                 Ok(v) => {
//                     if v.contains(&current) {
//                         client.push_play_queue(&v, current).await
//                     } else {
//                         return;
//                     }
//                 }
//                 Err(_) => return,
//             }
//         }
//         None => client.clear_play_queue().await,
//     };
//     if let Err(e) = result {
//         tracing::debug!("play queue sync to Tidal skipped: {e}");
//     }
// }
//
// #[cfg(test)]
// async fn sync_saved_queue(_ids: Option<(IdList, u64)>) {}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal route wrapping just savePlayQueue, so warp::test can drive
    // it without auth or the rest of the route table. The handler makes
    // no Tidal calls; assertions check status and error codes only, since
    // the store is process-wide and tests run in parallel.
    fn save_route() -> impl warp::Filter<Extract = (warp::reply::Json,), Error = warp::Rejection> + Clone {
        use warp::Filter;
        warp::path("rest")
            .and(warp::path("savePlayQueue"))
            .and(warp::path::end())
            .and(warp::query::<QueryParams>())
            .and_then(save_play_queue)
    }

    fn code_of<B: AsRef<[u8]>>(reply: &warp::http::Response<B>) -> u32 {
        let body: serde_json::Value = serde_json::from_slice(reply.body().as_ref()).unwrap();
        body["subsonic-response"]["error"]["code"].as_u64().unwrap() as u32
    }

    fn status_of<B: AsRef<[u8]>>(reply: &warp::http::Response<B>) -> String {
        let body: serde_json::Value = serde_json::from_slice(reply.body().as_ref()).unwrap();
        body["subsonic-response"]["status"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn save_without_current_fails_with_code_10() {
        let reply = warp::test::request()
            .path("/rest/savePlayQueue?id=t1&id=t2")
            .reply(&save_route())
            .await;
        assert_eq!(status_of(&reply), "failed");
        assert_eq!(code_of(&reply), 10);
    }

    #[tokio::test]
    async fn save_with_bad_id_fails_with_code_70() {
        let reply = warp::test::request()
            .path("/rest/savePlayQueue?id=al9&current=t1")
            .reply(&save_route())
            .await;
        assert_eq!(status_of(&reply), "failed");
        assert_eq!(code_of(&reply), 70);
    }

    #[tokio::test]
    async fn empty_save_returns_ok() {
        let reply = warp::test::request()
            .path("/rest/savePlayQueue")
            .reply(&save_route())
            .await;
        assert_eq!(status_of(&reply), "ok");
    }

    #[tokio::test]
    async fn valid_save_returns_ok() {
        let reply = warp::test::request()
            .path("/rest/savePlayQueue?id=t1&id=t2&current=t2&position=120000")
            .reply(&save_route())
            .await;
        assert_eq!(status_of(&reply), "ok");
    }

    // The ByIndex route, like save_route but for savePlayQueueByIndex.
    fn save_by_index_route(
    ) -> impl warp::Filter<Extract = (warp::reply::Json,), Error = warp::Rejection> + Clone {
        use warp::Filter;
        warp::path("rest")
            .and(warp::path("savePlayQueueByIndex"))
            .and(warp::path::end())
            .and(warp::query::<QueryParams>())
            .and_then(save_play_queue_by_index)
    }

    #[tokio::test]
    async fn by_index_valid_save_returns_ok() {
        let reply = warp::test::request()
            .path("/rest/savePlayQueueByIndex?id=t1&id=t2&id=t3&currentIndex=1&position=120000")
            .reply(&save_by_index_route())
            .await;
        assert_eq!(status_of(&reply), "ok");
    }

    #[tokio::test]
    async fn by_index_bad_id_fails_with_code_70() {
        let reply = warp::test::request()
            .path("/rest/savePlayQueueByIndex?id=ar7&currentIndex=0")
            .reply(&save_by_index_route())
            .await;
        assert_eq!(status_of(&reply), "failed");
        assert_eq!(code_of(&reply), 70);
    }

    #[tokio::test]
    async fn by_index_empty_save_returns_ok() {
        let reply = warp::test::request()
            .path("/rest/savePlayQueueByIndex")
            .reply(&save_by_index_route())
            .await;
        assert_eq!(status_of(&reply), "ok");
    }

    #[tokio::test]
    async fn by_index_without_current_index_is_tolerated() {
        let reply = warp::test::request()
            .path("/rest/savePlayQueueByIndex?id=t1&id=t2&position=5000")
            .reply(&save_by_index_route())
            .await;
        assert_eq!(status_of(&reply), "ok");
    }
}
