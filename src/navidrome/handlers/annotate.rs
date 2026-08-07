// Media annotation: scrobble. Playback reports from clients are
// acknowledged and logged; there is no Last.fm/ListenBrainz backend yet
// (TODO), so this is the future PlayReporter hook point.
use crate::navidrome::models::PingResponse;
use super::{fail, ok};
use crate::navidrome::params::QueryParams;

// scrobble: accept one or more playback reports. submission=false is a
// now-playing notification, submission=true a real scrobble. Missing id
// is a client error (code 10). Any id counts; Tidal tracks are not
// looked up here.
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
}
