// Media annotation: scrobble and now-playing reports. Playback reports
// are acknowledged and logged; there is no Last.fm/ListenBrainz backend
// yet (TODO), so this is the future PlayReporter hook point.
use crate::navidrome::models::PingResponse;
use crate::navidrome::now_playing;
use super::{fail, ok};
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::params::QueryParams;

// scrobble: accept one or more playback reports. submission=false is a
// now-playing notification, submission=true a real scrobble. Missing id
// is a client error (code 10). Any id counts; Tidal tracks are not
// looked up here. The latest report also feeds getNowPlaying; the entry
// expires after ten minutes.
pub async fn scrobble(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    if q.id.0.is_empty() {
        return Ok(fail(10, "Required parameter missing"));
    }
    for id in &q.id.0 {
        tracing::info!(
            "scrobble id={id} submission={} time={:?}",
            q.submission.unwrap_or(true),
            q.time
        );
    }
    if let Some(id) = q.id.0.last()
        && let Some(track_id) = ids::decode(IdKind::Track, id).or_else(|| id.parse().ok())
    {
        now_playing::report(track_id, q.u.clone().unwrap_or_default());
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
    if ids::decode(IdKind::Track, id).or_else(|| id.parse().ok()).is_none() {
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
