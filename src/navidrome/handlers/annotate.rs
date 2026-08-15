// Media annotation: scrobble and now-playing reports. The scrobble
// endpoint fans out to the configured reporters (Last.fm, ListenBrainz)
// via navidrome::scrobble; a failing backend only logs. Playback reports
// also feed getNowPlaying; a real scrobble (submission=true) does not.
use crate::navidrome::models::PingResponse;
use crate::navidrome::now_playing;
use crate::navidrome::scrobble;
use super::{fail, ok};
use crate::navidrome::ids;
use crate::navidrome::params::QueryParams;

// scrobble: accept one or more playback reports. submission=false is a
// now-playing notification, submission=true (the default) a real
// scrobble. Missing id is a client error (code 10). Any id counts;
// Tidal tracks are fetched only for real scrobbles. The latest
// now-playing report feeds getNowPlaying; the entry expires after ten
// minutes. Reporter failures never fail the request.
pub async fn scrobble(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    if q.id.0.is_empty() {
        return Ok(fail(10, "Required parameter missing"));
    }
    let submission = q.submission.unwrap_or(true);
    for id in &q.id.0 {
        tracing::info!(
            "scrobble id={id} submission={submission} time={:?}",
            q.time
        );
    }
    if !submission {
        // Now-playing notification: only the latest id feeds the slot;
        // the backends are told in the background.
        if let Some(id) = q.id.0.last()
            && let Some(track_id) = ids::parse_track_id(id)
        {
            now_playing::report(track_id, q.u.clone().unwrap_or_default());
            tokio::spawn(scrobble::report_now_playing(track_id));
        }
        return Ok(ok(PingResponse {}));
    }
    // Real scrobble: report every id. Track metadata is fetched through
    // the Tidal client; when it is unavailable (or a track is unknown),
    // that scrobble is skipped and logged, never a client error.
    let time_ms = q.time.unwrap_or_else(now_playing::now_ms);
    for id in &q.id.0 {
        let Some(track_id) = ids::parse_track_id(id) else {
            continue;
        };
        let Some(client) = crate::tidal::client_opt() else {
            tracing::warn!("scrobble id={id}: tidal client unavailable; skipped");
            continue;
        };
        let detail = match client.track(track_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("scrobble id={id}: track fetch failed: {e}");
                continue;
            }
        };
        let Some(song) = scrobble::scrobble_song_from_track(&detail) else {
            tracing::warn!("scrobble id={id}: track metadata incomplete; skipped");
            continue;
        };
        scrobble::report_song(&song, time_ms).await;
    }
    Ok(ok(PingResponse {}))
}

// setRating: acknowledge a rating write. Tidal has no rating backend
// for this account, so the reply is faked: validated and logged, then
// an empty ok. rating 0 removes the rating; 1-5 sets it.
pub async fn set_rating(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if ids::parse_track_id(id).is_none() {
        return Ok(fail(70, "Song not found"));
    }
    let Some(rating) = q.rating else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if rating > 5 {
        return Ok(fail(70, "Invalid rating"));
    }
    tracing::info!("setRating id={id} rating={rating}");
    Ok(ok(PingResponse {}))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal route wrapping just the scrobble handler, so warp::test
    // can drive it without auth or the rest of the route table.
    fn scrobble_route() -> impl warp::Filter<Extract = (warp::reply::Json,), Error = warp::Rejection> + Clone {
        use warp::Filter;
        warp::path("rest")
            .and(warp::path("scrobble"))
            .and(warp::path::end())
            .and(warp::query::<QueryParams>())
            .and_then(scrobble)
    }

    fn rating_route() -> impl warp::Filter<Extract = (warp::reply::Json,), Error = warp::Rejection> + Clone {
        use warp::Filter;
        warp::path("rest")
            .and(warp::path("setRating"))
            .and(warp::path::end())
            .and(warp::query::<QueryParams>())
            .and_then(set_rating)
    }

    fn body_of(reply: &warp::http::Response<impl AsRef<[u8]>>) -> serde_json::Value {
        serde_json::from_slice(reply.body().as_ref()).unwrap()
    }

    #[tokio::test]
    async fn missing_id_fails_with_code_10() {
        let reply = warp::test::request()
            .path("/rest/scrobble?submission=true")
            .reply(&scrobble_route())
            .await;
        let body: serde_json::Value = serde_json::from_slice(reply.body()).unwrap();
        assert_eq!(body["subsonic-response"]["status"], "failed");
        assert_eq!(body["subsonic-response"]["error"]["code"], 10);
    }

    #[tokio::test]
    async fn with_id_returns_ok() {
        let reply = warp::test::request()
            .path("/rest/scrobble?id=t463900374&submission=true&time=1786116785370")
            .reply(&scrobble_route())
            .await;
        let body: serde_json::Value = serde_json::from_slice(reply.body()).unwrap();
        let sr = &body["subsonic-response"];
        assert_eq!(sr["status"], "ok");
        assert!(sr.get("error").is_none());
    }

    // The lock intentionally covers the request await: the handler reads
    // and writes the shared now-playing slot.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn submission_false_feeds_now_playing() {
        let _g = crate::navidrome::now_playing::test_lock();
        // The Tidal client is unset in tests, so a real scrobble is
        // skipped; only the now-playing feed (submission=false) works.
        let reply = warp::test::request()
            .path("/rest/scrobble?id=t463900374&submission=false")
            .reply(&scrobble_route())
            .await;
        let body: serde_json::Value = serde_json::from_slice(reply.body()).unwrap();
        assert_eq!(body["subsonic-response"]["status"], "ok");
        let np = crate::navidrome::now_playing::current().unwrap();
        assert_eq!(np.track_id, 463900374);
        // Clear the slot before the next assertion.
        crate::navidrome::now_playing::report_playback(
            np.track_id,
            np.username.clone(),
            "stopped",
            0,
            1.0,
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn submission_true_does_not_feed_now_playing() {
        let _g = crate::navidrome::now_playing::test_lock();
        crate::navidrome::now_playing::report_playback(999, "admin".into(), "stopped", 0, 1.0);
        let reply = warp::test::request()
            .path("/rest/scrobble?id=t463900374&submission=true")
            .reply(&scrobble_route())
            .await;
        let body: serde_json::Value = serde_json::from_slice(reply.body()).unwrap();
        assert_eq!(body["subsonic-response"]["status"], "ok");
        assert!(
            crate::navidrome::now_playing::current().is_none(),
            "a real scrobble must not set the now-playing slot"
        );
    }

    #[tokio::test]
    async fn rating_without_id_fails_with_code_10() {
        let reply = warp::test::request()
            .path("/rest/setRating?rating=4")
            .reply(&rating_route())
            .await;
        let body = body_of(&reply);
        assert_eq!(body["subsonic-response"]["status"], "failed");
        assert_eq!(body["subsonic-response"]["error"]["code"], 10);
    }

    #[tokio::test]
    async fn valid_rating_returns_ok() {
        let reply = warp::test::request()
            .path("/rest/setRating?id=t42&rating=4")
            .reply(&rating_route())
            .await;
        let body = body_of(&reply);
        let sr = &body["subsonic-response"];
        assert_eq!(sr["status"], "ok");
        assert!(sr.get("error").is_none());
    }

    #[tokio::test]
    async fn rating_of_zero_removes_and_returns_ok() {
        let reply = warp::test::request()
            .path("/rest/setRating?id=t42&rating=0")
            .reply(&rating_route())
            .await;
        let body = body_of(&reply);
        assert_eq!(body["subsonic-response"]["status"], "ok");
    }

    #[tokio::test]
    async fn rating_above_five_fails_with_code_70() {
        let reply = warp::test::request()
            .path("/rest/setRating?id=t42&rating=6")
            .reply(&rating_route())
            .await;
        let body = body_of(&reply);
        assert_eq!(body["subsonic-response"]["status"], "failed");
        assert_eq!(body["subsonic-response"]["error"]["code"], 70);
    }

    #[tokio::test]
    async fn undecodable_id_fails_with_code_70() {
        let reply = warp::test::request()
            .path("/rest/setRating?id=abc&rating=3")
            .reply(&rating_route())
            .await;
        let body = body_of(&reply);
        assert_eq!(body["subsonic-response"]["status"], "failed");
        assert_eq!(body["subsonic-response"]["error"]["code"], 70);
    }
}
