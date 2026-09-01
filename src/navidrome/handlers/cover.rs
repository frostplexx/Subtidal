// Image serving: getAvatar (user avatar) and getCoverArt (302 redirects
// to Tidal's image CDN; the server never proxies image bytes).
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::params::QueryParams;
use super::{fail, redirect};
use crate::tidal::mapping::{artist_pic_url, cover_url, tidal_image_url};
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
    // A client may echo a coverArt URL (which is a full image URL on a
    // Tidal-owned host) back into the id parameter. Pass it through only
    // when the host is Tidal-owned; anything else is an attempted open
    // redirect.
    if id.starts_with("http") {
        if tidal_image_url(id) {
            // Rebuild the URL at the requested size; the artwork href
            // has a baked resolution (e.g. 1280x1280) that a 300px
            // request must not inherit.
            return Ok(redirect(cover_url(id, size)));
        }
        return Ok(fail(70, "Cover art not found").into_response());
    }
    // The id must be a UUID or a prefixed/bare Tidal id.
    let (uuid, artist_pic) = if id.contains('-') {
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
    Ok(redirect(url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navidrome::params::QueryParams;

    // The Location header of a getCoverArt reply for the given query.
    async fn location_for(query: &str) -> String {
        let q = QueryParams::from_merged(query).unwrap();
        let resp = get_cover_art(q).await.unwrap();
        resp.headers()
            .get("Location")
            .expect("redirect")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn echoed_image_url_is_resized_to_requested_size() {
        // A playlist cover echoed back with a baked 320x320 must serve
        // the exact requested 80 (which the CDN has), not 160.
        let loc = location_for(
            "id=https://resources.tidal.com/images/3536e16f/a438/4fdf/8935/e58e8bb4a68b/320x320.jpg&u=admin&s=olZ9Uk6yHaPd&t=x&v=1.13.0&c=Feishin&size=80",
        )
        .await;
        assert_eq!(
            loc,
            "https://resources.tidal.com/images/3536e16f/a438/4fdf/8935/e58e8bb4a68b/80x80.jpg"
        );
    }

    #[tokio::test]
    async fn echoed_oversized_image_gets_downscaled() {
        let loc = location_for(
            "id=https://resources.tidal.com/images/3536e16f/a438/4fdf/8935/e58e8bb4a68b/1280x1280.jpg&u=admin&s=x&t=x&v=1.13.0&c=Feishin&size=300",
        )
        .await;
        assert_eq!(
            loc,
            "https://resources.tidal.com/images/3536e16f/a438/4fdf/8935/e58e8bb4a68b/320x320.jpg"
        );
    }
}
