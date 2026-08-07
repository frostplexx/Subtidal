// Now-playing: updateNowPlaying reports playback, getNowPlaying serves the
// latest report. updateNowPlaying is the legacy endpoint (scrobble with
// submission=false is its OpenSubsonic replacement); VeloSonic still calls
// it, so it is aliased here.
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
// full song plus the report metadata; minutesAgo counts from the report.
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
    let minutes_ago = ((now_playing::now_ms() - np.started_ms).max(0) / 60_000) as u32;
    Ok(ok(NowPlayingResponse {
        now_playing: NowPlaying {
            entry: vec![NowPlayingEntry {
                song,
                username: np.username,
                minutes_ago,
                player_id: 0,
            }],
        },
    }))
}
