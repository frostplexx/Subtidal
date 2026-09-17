// Image serving: getAvatar (user avatar) and getCoverArt (302 redirects
// to Tidal's image CDN; the server never proxies image bytes).
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::params::QueryParams;
use super::{fail, fail_with, redirect};
use crate::tidal::mapping::{artist_pic_url, cover_cache, cover_url};
use warp::Reply;

// getAvatar: the Tidal account avatar, when one is set, else a neutral
// placeholder PNG. The avatar 302-redirects to the image CDN (zero server
// bandwidth); a missing picture falls back to the embedded placeholder.
pub async fn get_avatar(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
    let size = q.size.unwrap_or(640);
    match crate::tidal::client().user_profile().await {
        Ok(v) => {
            if let Some(pic) = v["picture"].as_str().filter(|s| !s.is_empty()) {
                let url = cover_url(pic, size);
                return Ok(redirect(url));
            }
        }
        Err(e) => tracing::warn!("user profile fetch failed: {e}"),
    }
    Ok(warp::reply::with_header(
        warp::reply::with_status(PLACEHOLDER_PNG, warp::http::StatusCode::OK),
        "Content-Type",
        "image/png",
    )
    .into_response())
}

// A 1x1 transparent PNG; the fallback avatar.
// TODO: Replace this with a more neutral placeholder (e.g. a gray silhouette) to avoid
// confusion with a missing image.
const PLACEHOLDER_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
    0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 11, 73, 68, 65, 84, 120, 156, 99, 96, 0, 2, 0, 0, 5, 0,
    1, 122, 94, 171, 63, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
pub async fn get_cover_art(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing").into_response());
    };
    let size = q.size.unwrap_or(640);
    // A mix id: refetch the mixes page (mixes carry no standalone lookup)
    // and pull the matching card's image, like getPlaylist does for a mix.
    // Skipped when a prior list/mapping call already cached this mix's
    // image, which is the common case (mixes are always listed first).
    if let Some(mix_id) = id.strip_prefix("mx") {
        if let Some(url) = cover_cache::lookup(id) {
            return Ok(redirect(cover_url(&url, size)));
        }
        let list = match crate::tidal::client().my_mixes().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("mixes fetch failed: {e}");
                return Ok(fail_with(0, "Cover art unavailable", &e).into_response());
            }
        };
        let Some(mix) = crate::tidal::mapping::mixes_from_page(&list)
            .into_iter()
            .find(|m| m["id"].as_str() == Some(mix_id))
        else {
            return Ok(fail(70, "Cover art not found").into_response());
        };
        let Some(url) = mix["images"]["MEDIUM"]["url"]
            .as_str()
            .or_else(|| mix["images"]["SMALL"]["url"].as_str())
        else {
            return Ok(fail(70, "Cover art not found").into_response());
        };
        cover_cache::remember(id, url);
        return Ok(redirect(cover_url(url, size)));
    }
    // The id must be a UUID or a prefixed/bare Tidal id. A cache hit (from
    // a prior list/mapping call) skips the per-item detail fetch below.
    let (uuid, artist_pic) = if let Some(cached) = cover_cache::lookup(id) {
        (Some(cached), id.starts_with("ar"))
    } else if id.contains('-') {
        // Bare UUID: a playlist id. Playlist covers come from squareImage.
        let result = match crate::tidal::client().playlist(id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("playlist fetch failed: {e}");
                return Ok(fail_with(0, "Cover art unavailable", &e).into_response());
            }
        };
        let cover = result["squareImage"]
            .as_str()
            .or_else(|| result["image"].as_str());
        if let Some(c) = cover {
            cover_cache::remember(id, c);
        }
        (cover.map(String::from), false)
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
                Ok(v) => {
                    let cover = v["cover"].as_str();
                    if let Some(c) = cover {
                        cover_cache::remember(id, c);
                    }
                    (cover.map(String::from), false)
                }
                Err(e) => {
                    tracing::warn!("album fetch failed: {e}");
                    return Ok(fail_with(0, "Cover art unavailable", &e).into_response());
                }
            },
            IdKind::Artist => match crate::tidal::client().artist(raw_id).await {
                Ok(v) => {
                    let pic = v["picture"].as_str();
                    if let Some(p) = pic {
                        cover_cache::remember(id, p);
                    }
                    (pic.map(String::from), true)
                }
                Err(e) => {
                    tracing::warn!("artist fetch failed: {e}");
                    return Ok(fail_with(0, "Cover art unavailable", &e).into_response());
                }
            },
            // A track id resolves through its album: some clients pass
            // the song id rather than the coverArt id they were given.
            IdKind::Track => match crate::tidal::client().track(raw_id).await {
                Ok(t) => {
                    let json = t.to_json();
                    let cover = json["album"]["cover"].as_str();
                    if let Some(c) = cover {
                        cover_cache::remember(id, c);
                    }
                    (cover.map(String::from), false)
                }
                Err(e) => {
                    tracing::warn!("track fetch failed: {e}");
                    return Ok(fail_with(0, "Cover art unavailable", &e).into_response());
                }
            },
            IdKind::Playlist => (None, false),
        }
    };
    let Some(uuid) = uuid else {
        return Ok(fail(70, "Cover art not found").into_response());
    };
    let url = if artist_pic { artist_pic_url(&uuid, size) } else { cover_url(&uuid, size) };
    Ok(redirect(url))
}
