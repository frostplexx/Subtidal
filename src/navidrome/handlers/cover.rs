// getCoverArt: resolve a cover id to a Tidal image URL and 302-redirect
// there. The server never proxies image bytes. Accepted ids:
//   - a full image URL (redirect straight through)
//   - a bare UUID (playlist cover; playlists use UUID ids)
//   - al<id> / ar<id> / bare album number
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::params::QueryParams;
use super::fail;
use crate::tidal::mapping::{artist_pic_url, cover_url};
use warp::Reply;
pub async fn get_cover_art(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing").into_response());
    };
    let size = q.size.unwrap_or(640);
    let (uuid, artist_pic) = if id.starts_with("http") {
        (Some(id.clone()), false)
    } else if id.contains('-') {
        // Bare UUID: a playlist id. Playlist covers come from squareImage.
        let result = match crate::tidal::client().playlist(id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("playlist fetch failed: {e}");
                return Ok(fail(0, "Cover art unavailable").into_response());
            }
        };
        (
            result["squareImage"]
                .as_str()
                .or_else(|| result["image"].as_str())
                .map(String::from),
            false,
        )
    } else {
        let (kind, raw_id) = match ids::parse(id) {
            Some(kv) => kv,
            // Bare number = raw Tidal album ID (Subsonic convention).
            None => match id.parse::<u64>() {
                Ok(n) => (IdKind::Album, n),
                Err(_) => return Ok(fail(70, "Cover art not found").into_response()),
            },
        };
        match kind {
            IdKind::Album => match crate::tidal::client().album(raw_id).await {
                Ok(v) => (v["cover"].as_str().map(String::from), false),
                Err(e) => {
                    tracing::warn!("album fetch failed: {e}");
                    return Ok(fail(0, "Cover art unavailable").into_response());
                }
            },
            IdKind::Artist => match crate::tidal::client().artist(raw_id).await {
                Ok(v) => (v["picture"].as_str().map(String::from), true),
                Err(e) => {
                    tracing::warn!("artist fetch failed: {e}");
                    return Ok(fail(0, "Cover art unavailable").into_response());
                }
            },
            // Tracks carry no own cover.
            _ => (None, false),
        }
    };
    let Some(uuid) = uuid else {
        return Ok(fail(70, "Cover art not found").into_response());
    };
    let url = if artist_pic { artist_pic_url(&uuid, size) } else { cover_url(&uuid, size) };
    Ok(warp::reply::with_status(
        warp::reply::with_header(warp::reply(), "Location", url),
        warp::http::StatusCode::FOUND,
    )
    .into_response())
}
