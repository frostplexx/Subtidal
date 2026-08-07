// Track browsing: getSong and the rest of the single-track endpoints.
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::GetSongResponse;
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::{song_from_track, year_from};

// getSong: one track's detail. The id may be t<id> or a bare number.
// Tidal track JSON carries no release date (not even on the embedded
// album), so the year is filled from the album detail, mirroring getAlbum.
// The album fetch hits the meta cache, so repeat calls cost nothing.
pub async fn get_song(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::decode(IdKind::Track, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Song not found"));
    };
    let client = crate::tidal::client();
    let detail = match client.track(track_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal track fetch failed: {e}");
            return Ok(fail(0, "Song unavailable"));
        }
    };
    let mut song = match song_from_track(&detail) {
        Some(s) => s,
        None => return Ok(fail(70, "Song not found")),
    };
    if song.year.is_none()
        && let Some(album_id) = detail["album"]["id"].as_u64()
        && let Ok(album) = client.album(album_id).await
    {
        song.year = year_from(album["releaseDate"].as_str());
    }
    Ok(ok(GetSongResponse { song }))
}
