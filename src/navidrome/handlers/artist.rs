// Artist browsing: getArtist, getTopSongs, and getArtistInfo2.
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::{
    AlbumId3, ArtistInfo2, ArtistInfo2Response, ArtistWithAlbums, Child, GetArtistResponse,
    TopSongs, TopSongsResponse,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::{
    album_from_tidal, artist_from_tidal, artist_pic_url, search_items, song_from_track,
};

// getArtist: one artist plus their albums. Tidal reports no albumCount on
// the detail, so the count is the number of albums returned.
pub async fn get_artist(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    // ar<id>, or a bare number as a raw Tidal artist id.
    let Some(artist_id) = ids::decode(IdKind::Artist, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Artist not found"));
    };
    let client = crate::tidal::client();
    let detail = match client.artist(artist_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal artist fetch failed: {e}");
            return Ok(fail(0, "Artist unavailable"));
        }
    };
    let albums = match client.artist_albums(artist_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal artist albums fetch failed: {e}");
            return Ok(fail(0, "Artist unavailable"));
        }
    };
    let mut artist = match artist_from_tidal(&detail) {
        Some(a) => a,
        None => return Ok(fail(70, "Artist not found")),
    };
    let album: Vec<AlbumId3> = albums["items"]
        .as_array()
        .map(|items| items.iter().filter_map(album_from_tidal).collect())
        .unwrap_or_default();
    artist.album_count = Some(album.len() as u32);
    Ok(ok(GetArtistResponse {
        artist: ArtistWithAlbums { artist, album },
    }))
}

// getTopSongs: an artist's most popular tracks. The id param wins when
// present (the topSongsByArtistId extension); a bare artist name resolves
// through search. count defaults to 50 per the spec.
pub async fn get_top_songs(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let client = crate::tidal::client();
    let artist_id = match q.id.0.first() {
        Some(id) => match ids::decode(IdKind::Artist, id).or_else(|| id.parse().ok()) {
            Some(n) => n,
            None => return Ok(fail(70, "Artist not found")),
        },
        None => match q.artist.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => {
                let result = match client.search(name).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("tidal artist search failed: {e}");
                        return Ok(fail(0, "Top songs unavailable"));
                    }
                };
                match search_items(&result, "artists")
                    .first()
                    .and_then(|a| a["id"].as_u64())
                {
                    Some(n) => n,
                    None => return Ok(fail(70, "Artist not found")),
                }
            }
            _ => return Ok(fail(10, "Required parameter missing")),
        },
    };
    let count = q.count.unwrap_or(50).min(500);
    let result = match client.artist_top_tracks(artist_id, count).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal top tracks fetch failed: {e}");
            return Ok(fail(0, "Top songs unavailable"));
        }
    };
    let song: Vec<Child> = result["items"]
        .as_array()
        .map(|items| items.iter().filter_map(song_from_track).collect())
        .unwrap_or_default();
    Ok(ok(TopSongsResponse {
        top_songs: TopSongs { song },
    }))
}

// getArtistInfo2: biography, portraits, and similar artists. The id may be
// an artist, album, or song id; albums and songs resolve through their first
// artist. The bio carries [wimpLink ...] wiki markup, which gets stripped.
pub async fn get_artist_info2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let client = crate::tidal::client();
    let artist_id = match ids::parse(id) {
        Some((IdKind::Artist, n)) => n,
        Some((IdKind::Album, n)) => match client.album(n).await {
            Ok(v) => match v["artists"].get(0).and_then(|a| a["id"].as_u64()) {
                Some(a) => a,
                None => return Ok(fail(70, "Artist not found")),
            },
            Err(e) => {
                tracing::error!("tidal album fetch failed: {e}");
                return Ok(fail(0, "Artist info unavailable"));
            }
        },
        Some((IdKind::Track, n)) => match client.track(n).await {
            Ok(v) => match v["artists"].get(0).and_then(|a| a["id"].as_u64()) {
                Some(a) => a,
                None => return Ok(fail(70, "Artist not found")),
            },
            Err(e) => {
                tracing::error!("tidal track fetch failed: {e}");
                return Ok(fail(0, "Artist info unavailable"));
            }
        },
        _ => match id.parse().ok() {
            Some(n) => n,
            None => return Ok(fail(70, "Artist not found")),
        },
    };
    let count = q.count.unwrap_or(20).min(500);
    let detail = match client.artist(artist_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal artist fetch failed: {e}");
            return Ok(fail(0, "Artist info unavailable"));
        }
    };
    let bio = match client.artist_bio(artist_id).await {
        Ok(v) => v["text"].as_str().unwrap_or("").to_string(),
        Err(e) => {
            tracing::error!("tidal bio fetch failed: {e}");
            return Ok(fail(0, "Artist info unavailable"));
        }
    };
    let similar = match client.artist_similar(artist_id, count).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal similar artists fetch failed: {e}");
            return Ok(fail(0, "Artist info unavailable"));
        }
    };
    let picture = detail["picture"].as_str();
    Ok(ok(ArtistInfo2Response {
        artist_info: ArtistInfo2 {
            biography: strip_wimplinks(&bio),
            music_brainz_id: String::new(),
            last_fm_url: String::new(),
            small_image_url: picture.map(|p| artist_pic_url(p, 160)),
            medium_image_url: picture.map(|p| artist_pic_url(p, 480)),
            large_image_url: picture.map(|p| artist_pic_url(p, 750)),
            similar_artist: similar["items"]
                .as_array()
                .map(|items| items.iter().filter_map(artist_from_tidal).collect())
                .unwrap_or_default(),
        },
    }))
}

// Strip [wimpLink artistId=...]...[/wimpLink] wiki markup from bio text.
// Opening tags carry attributes, so each is skipped to its closing bracket;
// closing tags are removed wherever they appear.
fn strip_wimplinks(text: &str) -> String {
    const CLOSE: &str = "[/wimpLink]";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let open = rest.find("[wimpLink");
        let close = rest.find(CLOSE);
        match (open, close) {
            (Some(o), Some(c)) if o < c => {
                out.push_str(&rest[..o]);
                rest = &rest[o + 1..];
                if let Some(end) = rest.find(']') {
                    rest = &rest[end + 1..];
                }
            }
            (Some(_), Some(c)) => {
                out.push_str(&rest[..c]);
                rest = &rest[c + CLOSE.len()..];
            }
            (Some(o), None) => {
                out.push_str(&rest[..o]);
                rest = &rest[o + 1..];
                if let Some(end) = rest.find(']') {
                    rest = &rest[end + 1..];
                }
            }
            (None, Some(c)) => {
                out.push_str(&rest[..c]);
                rest = &rest[c + CLOSE.len()..];
            }
            (None, None) => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::strip_wimplinks;

    #[test]
    fn strip_wimplinks_removes_all_markup() {
        let bio = "[wimpLink artistId=\"1\"]A[/wimpLink] and "
            .to_string()
            + "[wimpLink artistId=\"2\"]B[/wimpLink] here, [wimpLink]C[/wimpLink] done.";
        assert_eq!(strip_wimplinks(&bio), "A and B here, C done.");
        assert_eq!(strip_wimplinks("no markup"), "no markup");
        assert_eq!(strip_wimplinks(""), "");
    }
}
