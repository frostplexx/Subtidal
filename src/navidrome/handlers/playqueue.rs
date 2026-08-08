// Play queue: savePlayQueue saves the queue, getPlayQueue returns it
// with fresh song detail. The store is in memory (play_state), so a
// restart clears it. Song ids are fetched one at a time; Tidal offers no
// batch track endpoint.
use crate::navidrome::ids;
use crate::navidrome::models::{Child, PingResponse, PlayQueue, PlayQueueResponse};
use crate::navidrome::now_playing::now_ms;
use crate::navidrome::params::QueryParams;
use crate::navidrome::play_state;
use crate::tidal::mapping::song_from_track;
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
    tracing::info!("savePlayQueue {} songs", q.id.0.len());
    Ok(ok(PingResponse {}))
}

// getPlayQueue: the saved queue, or an empty one when nothing is saved.
// Tracks that Tidal no longer serves are dropped from the reply.
pub async fn get_play_queue(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let client = crate::tidal::client();
    let saved = match play_state::queue() {
        Some(q) => q,
        None => return Ok(ok(PlayQueueResponse { play_queue: empty_queue() })),
    };
    let mut entry: Vec<Child> = Vec::new();
    for track_id in &saved.track_ids {
        match client.track(*track_id).await {
            Ok(v) => {
                if let Some(song) = song_from_track(&v) {
                    entry.push(song);
                }
            }
            Err(e) => {
                tracing::debug!("tidal track fetch failed (dropped from queue): {e}");
            }
        }
    }
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

fn empty_queue() -> PlayQueue {
    PlayQueue {
        current: None,
        position: 0,
        username: String::new(),
        changed: play_state::iso8601_z(now_ms()),
        changed_by: String::new(),
        entry: vec![],
    }
}

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
}
