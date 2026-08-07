// Now-playing: updateNowPlaying and reportPlayback report playback,
// getNowPlaying serves the latest report. updateNowPlaying is the legacy
// endpoint (scrobble with submission=false is its OpenSubsonic
// replacement); VeloSonic still calls it, so it is aliased here.
// reportPlayback is the playbackReport extension: full timeline states.
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::{NowPlaying, NowPlayingEntry, NowPlayingResponse, PingResponse};
use crate::navidrome::now_playing;
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::song_from_track;

// updateNowPlaying: record the reported song as currently playing.
pub async fn update_now_playing(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::decode(IdKind::Track, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Song not found"));
    };
    now_playing::report(track_id, q.u.clone().unwrap_or_default());
    tracing::info!("updateNowPlaying id={id}");
    Ok(ok(PingResponse {}))
}

// getNowPlaying: the latest report, if it is still fresh. The entry is a
// full song plus the report metadata; minutesAgo counts from the session
// start and positionMs is estimated forward from the last report while
// playing.
pub async fn get_now_playing(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(np) = now_playing::current() else {
        return Ok(ok(NowPlayingResponse {
            now_playing: NowPlaying { entry: vec![] },
        }));
    };
    let client = crate::tidal::client();
    let detail = match client.track(np.track_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal track fetch failed: {e}");
            return Ok(fail(0, "Now playing unavailable"));
        }
    };
    let Some(song) = song_from_track(&detail) else {
        return Ok(fail(70, "Song not found"));
    };
    let now = now_playing::now_ms();
    let minutes_ago = ((now - np.started_ms).max(0) / 60_000) as u32;
    let position_ms = now_playing::position_at(&np, now);
    Ok(ok(NowPlayingResponse {
        now_playing: NowPlaying {
            entry: vec![NowPlayingEntry {
                song,
                username: np.username,
                minutes_ago,
                player_id: 0,
                state: np.state,
                position_ms,
                playback_rate: np.playback_rate,
            }],
        },
    }))
}

// reportPlayback: apply a playback timeline event to the now-playing
// state. stopped clears it; starting, playing, and paused update it. A
// stopped report with scrobbling enabled is the completion signal; the
// PlayReporter backend (TODO) hooks here.
pub async fn report_playback(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.media_id.as_deref() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if q.media_type.as_deref() != Some("song") {
        return Ok(fail(0, "Unsupported media type"));
    }
    let Some(position_ms) = q.position_ms else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(state) = q.state.as_deref() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::decode(IdKind::Track, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Song not found"));
    };
    // Map the wire state onto a 'static string for the shared slot.
    let state: &'static str = match state {
        "starting" => "starting",
        "playing" => "playing",
        "paused" => "paused",
        "stopped" => "stopped",
        _ => return Ok(fail(70, "Invalid state")),
    };
    let rate = q.playback_rate.unwrap_or(1.0);
    let ignore = q.ignore_scrobble.unwrap_or(false);
    tracing::info!(
        "reportPlayback id={id} state={state} positionMs={position_ms} playbackRate={rate} ignoreScrobble={ignore}"
    );
    if state == "stopped" && !ignore {
        tracing::info!("scrobble (completed) id={id}");
    }
    now_playing::report_playback(
        track_id,
        q.u.clone().unwrap_or_default(),
        state,
        position_ms,
        rate,
    );
    Ok(ok(PingResponse {}))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal route wrapping just the reportPlayback handler, so
    // warp::test can drive it without auth or the rest of the route
    // table. The handler makes no Tidal calls.
    fn report_route() -> impl warp::Filter<Extract = (warp::reply::Json,), Error = warp::Rejection> + Clone {
        use warp::Filter;
        warp::path("rest")
            .and(warp::path("reportPlayback"))
            .and(warp::path::end())
            .and(warp::query::<QueryParams>())
            .and_then(report_playback)
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
    async fn missing_media_id_fails_with_code_10() {
        let reply = warp::test::request()
            .path("/rest/reportPlayback?mediaType=song&positionMs=120000&state=playing")
            .reply(&report_route())
            .await;
        assert_eq!(status_of(&reply), "failed");
        assert_eq!(code_of(&reply), 10);
    }

    #[tokio::test]
    async fn unknown_state_fails_with_code_70() {
        let reply = warp::test::request()
            .path("/rest/reportPlayback?mediaId=t42&mediaType=song&positionMs=120000&state=buffering")
            .reply(&report_route())
            .await;
        assert_eq!(status_of(&reply), "failed");
        assert_eq!(code_of(&reply), 70);
    }

    #[tokio::test]
    async fn valid_report_returns_ok() {
        let reply = warp::test::request()
            .path("/rest/reportPlayback?mediaId=t42&mediaType=song&positionMs=120000&state=playing&playbackRate=1.0&ignoreScrobble=false")
            .reply(&report_route())
            .await;
        assert_eq!(status_of(&reply), "ok");
    }

    #[tokio::test]
    async fn non_song_media_type_fails() {
        let reply = warp::test::request()
            .path("/rest/reportPlayback?mediaId=t42&mediaType=podcast&positionMs=120000&state=playing")
            .reply(&report_route())
            .await;
        assert_eq!(status_of(&reply), "failed");
    }
}
