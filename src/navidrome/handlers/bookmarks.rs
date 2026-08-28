// Bookmarks: positions inside tracks, stored in memory (play_state).
// getBookmarks lists them with fresh song detail; createBookmark upserts,
// deleteBookmark removes. Song detail is fetched one track at a time;
// Tidal offers no batch track endpoint.
use crate::navidrome::ids;
use crate::navidrome::models::{Bookmark, Bookmarks, BookmarksResponse, PingResponse};
use crate::navidrome::now_playing::now_ms;
use crate::navidrome::params::QueryParams;
use crate::navidrome::play_state;
use crate::tidal::mapping::song_from_track;
use super::{fail, ok};

// createBookmark: set or overwrite the position for one song.
pub async fn create_bookmark(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found"));
    };
    let Some(position) = q.position else {
        return Ok(fail(10, "Required parameter missing"));
    };
    play_state::upsert_bookmark(
        track_id,
        position,
        q.comment.clone().unwrap_or_default(),
        q.u.clone().unwrap_or_default(),
        now_ms(),
    );
    tracing::info!("createBookmark id={id} position={position}");
    Ok(ok(PingResponse {}))
}

// deleteBookmark: remove the bookmark for one song. A missing bookmark
// is not an error, matching unstar.
pub async fn delete_bookmark(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found"));
    };
    play_state::delete_bookmark(track_id);
    tracing::info!("deleteBookmark id={id}");
    Ok(ok(PingResponse {}))
}

// getBookmarks: all bookmarks, oldest first. Tracks that Tidal no longer
// serves are dropped from the reply.
pub async fn get_bookmarks(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let client = crate::tidal::client();
    let saved = play_state::bookmarks();
    let mut bookmark: Vec<Bookmark> = Vec::new();
    for b in saved {
        let entry = match client.track(b.track_id).await {
            Ok(v) => song_from_track(&v.to_json()),
            Err(e) => {
                tracing::debug!("tidal track fetch failed (bookmark dropped): {e}");
                None
            }
        };
        if let Some(entry) = entry {
            bookmark.push(Bookmark {
                entry,
                position: b.position_ms,
                username: b.username,
                comment: b.comment,
                created: play_state::iso8601_z(b.created_ms),
                changed: play_state::iso8601_z(b.changed_ms),
            });
        }
    }
    Ok(ok(BookmarksResponse {
        bookmarks: Bookmarks { bookmark },
    }))
}
