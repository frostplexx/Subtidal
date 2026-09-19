// Artist browsing: getArtist, getTopSongs, getArtistInfo, and getArtistInfo2.
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::{
    AlbumId3, ArtistInfo2, ArtistInfo2Response, ArtistInfoResponse, ArtistWithAlbums, Child,
    GetArtistResponse, TopSongs, TopSongsResponse,
};
use crate::navidrome::params::QueryParams;
use super::{fail, fail_with, ok};
use crate::tidal::mapping::{
    album_from_tidal, artist_from_tidal, artist_pic_url, search_items, song_from_track,
};

// getArtist: one artist plus their albums. Tidal reports no albumCount on
// the detail, so the count is the number of albums returned. The v2
// detail carries the portrait; the v1 object is fetched alongside for
// its `artistRoles`, which v2 lacks. The v1 fetch is best-effort: the
// roles list is just empty without it.
pub async fn get_artist(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    // ar<id>, or a bare number as a raw Tidal artist id.
    let Some(artist_id) = ids::decode(IdKind::Artist, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Artist not found"));
    };
    let client = crate::tidal::client();
    let (detail, albums, v1) = tokio::join!(
        client.artist(artist_id),
        client.artist_albums(artist_id),
        client.artist_v1(artist_id)
    );
    let mut detail = match detail {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal artist fetch failed: {e}");
            return Ok(fail_with(0, "Artist unavailable", &e));
        }
    };
    let albums = match albums {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal artist albums fetch failed: {e}");
            return Ok(fail_with(0, "Artist unavailable", &e));
        }
    };
    match v1 {
        Ok(v) if v["artistRoles"].is_array() => detail["artistRoles"] = v["artistRoles"].clone(),
        Ok(_) => {}
        Err(e) => tracing::debug!("tidal v1 artist fetch failed: {e}"),
    }
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
                        return Ok(fail_with(0, "Top songs unavailable", &e));
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
    let result = match crate::tidal::client::TidalClient::artist_top_tracks_parallel(client, artist_id, count)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal top tracks fetch failed: {e}");
            return Ok(fail_with(0, "Top songs unavailable", &e));
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

// getArtistInfo: the classic variant. Identical payload to getArtistInfo2,
// wrapped under the artistInfo name. The id may be an artist, album, or
// song id; albums and songs resolve through their first artist.
pub async fn get_artist_info(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match artist_info(q).await {
        Ok(info) => Ok(ok(ArtistInfoResponse { artist_info: info })),
        Err(f) => Ok(f),
    }
}

// getArtistInfo2: biography, portraits, and similar artists. The id may be
// an artist, album, or song id; albums and songs resolve through their first
// artist. The bio carries [wimpLink ...] wiki markup, which gets stripped.
pub async fn get_artist_info2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    match artist_info(q).await {
        Ok(info) => Ok(ok(ArtistInfo2Response { artist_info: info })),
        Err(f) => Ok(f),
    }
}

async fn artist_info(q: QueryParams) -> Result<ArtistInfo2, warp::reply::Json> {
    let Some(id) = q.id.0.first() else {
        return Err(fail(10, "Required parameter missing"));
    };
    let client = crate::tidal::client();
    let artist_id = match ids::parse(id) {
        Some((IdKind::Artist, n)) => n,
        Some((IdKind::Album, n)) => match client.album(n).await {
            Ok(v) => match v["artists"].get(0).and_then(|a| a["id"].as_u64()) {
                Some(a) => a,
                None => return Err(fail(70, "Artist not found")),
            },
            Err(e) => {
                tracing::error!("tidal album fetch failed: {e}");
                return Err(fail_with(0, "Artist info unavailable", &e));
            }
        },
        Some((IdKind::Track, n)) => match client.track(n).await {
            Ok(v) => match v.artists.as_ref().and_then(|a| a.first()).map(|a| a.id) {
                Some(a) => a,
                None => return Err(fail(70, "Artist not found")),
            },
            Err(e) => {
                tracing::error!("tidal track fetch failed: {e}");
                return Err(fail_with(0, "Artist info unavailable", &e));
            }
        },
        _ => match id.parse().ok() {
            Some(n) => n,
            None => return Err(fail(70, "Artist not found")),
        },
    };
    let count = q.count.unwrap_or(20).min(500);
    // The artist itself must resolve; the bio and similar artists are
    // extras that many artists simply lack (Tidal 404s), so either one
    // failing degrades to an empty field rather than losing the images.
    let (detail, bio, similar) = tokio::join!(
        client.artist(artist_id),
        client.artist_bio(artist_id),
        client.artist_similar(artist_id, count),
    );
    let detail = match detail {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal artist fetch failed: {e}");
            return Err(fail_with(0, "Artist info unavailable", &e));
        }
    };
    let bio = match bio {
        Ok(v) => v["text"].as_str().unwrap_or("").to_string(),
        Err(e) => {
            tracing::debug!("tidal bio fetch failed for {artist_id}: {e}");
            String::new()
        }
    };
    let similar = match similar {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("tidal similar artists fetch failed for {artist_id}: {e}");
            serde_json::json!({ "items": [] })
        }
    };
    let picture = detail["picture"].as_str();
    Ok(ArtistInfo2 {
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
    })
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
