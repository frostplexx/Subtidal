// Favorites: getStarred / getStarred2. Both read the same three Tidal
// favorites lists; the difference is only the wrapping. getStarred uses
// legacy shapes (isDir albums, minimal artists); getStarred2 the ID3 shapes.
use crate::navidrome::models::{
    Child, PingResponse, Starred, Starred2, Starred2Album, Starred2Artist, Starred2Response,
    StarredAlbum, StarredResponse,
};
use crate::navidrome::ids;
use crate::navidrome::params::QueryParams;
use crate::tidal::client::FavoriteKind;
use super::{fail, ok};
use crate::tidal::mapping::{
    artist_from_tidal, favorite_album_from_tidal, favorite_artist_from_tidal, song_from_track,
};

// Map a Tidal favorites response to AlbumID3 items.
pub(crate) fn favorites_albums(result: &serde_json::Value) -> Vec<crate::navidrome::models::AlbumId3> {
    result["items"]
        .as_array()
        .map(|items| items.iter().filter_map(favorite_album_from_tidal).collect())
        .unwrap_or_default()
}

// Map a favorites track list to Child, carrying the Tidal favorite time on `created`, `starred`, and `starredAt`.
pub(crate) fn favorite_track_songs(result: &serde_json::Value) -> Vec<Child> {
    result["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let mut song = song_from_track(&entry["item"])?;
                    song.created = entry["created"].as_str().unwrap_or("").to_string();
                    song.starred = entry["created"].as_str().map(String::from);
                    song.starred_at = entry["created"].as_str().map(String::from);
                    Some(song)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn fetch_favorites() -> super::BoxedTryFuture<
    (serde_json::Value, serde_json::Value, serde_json::Value),
    (),
> {
    Box::pin(async move {
    let client = crate::tidal::client();
    let albums = client.favorite_albums(0, 2000);
    let artists = client.favorite_artists(0, 2000);
    let tracks = crate::tidal::client::TidalClient::favorite_tracks_parallel(client);
    let (albums, artists, tracks) = match tokio::try_join!(albums, artists, tracks) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal favorites fetch failed: {e}");
            return Err(());
        }
    };
    Ok((albums, artists, tracks))
    })
}

pub async fn get_starred() -> Result<warp::reply::Json, warp::Rejection> {
    let (albums, artists, tracks) = match fetch_favorites().await {
        Ok(v) => v,
        Err(()) => return Ok(fail(0, "Favorites unavailable")),
    };
    let album = favorites_albums(&albums)
        .into_iter()
        .map(|a| StarredAlbum {
            parent: a.artist_id.clone(),
            is_dir: true,
            starred: a.created.clone(),
            album: a,
        })
        .collect();
    let artist = artists["items"]
        .as_array()
        .map(|items| items.iter().filter_map(favorite_artist_from_tidal).collect())
        .unwrap_or_default();
    Ok(ok(StarredResponse {
        starred: Starred {
            artist,
            album,
            song: favorite_track_songs(&tracks),
        },
    }))
}

pub async fn get_starred2() -> Result<warp::reply::Json, warp::Rejection> {
    let (albums, artists, tracks) = match fetch_favorites().await {
        Ok(v) => v,
        Err(()) => return Ok(fail(0, "Favorites unavailable")),
    };
    let album = favorites_albums(&albums)
        .into_iter()
        .map(|a| Starred2Album {
            starred: a.created.clone(),
            album: a,
        })
        .collect();
    let artist = artists["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let mut artist = artist_from_tidal(&entry["item"])?;
                    artist.starred = entry["created"].as_str().map(String::from);
                    artist.starred_at = entry["created"].as_str().map(String::from);
                    Some(Starred2Artist {
                        artist_image_url: artist.cover_art.clone(),
                        starred: entry["created"].as_str().map(String::from),
                        artist,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ok(Starred2Response {
        starred2: Starred2 {
            artist,
            album,
            song: favorite_track_songs(&tracks),
        },
    }))
}

// star / unstar: toggle favorites. Both accept id (t<id>, al<id>, ar<id>
// or a bare track number), albumId and artistId; several of each allowed.
// Tidal mutations are idempotent, so repeat heart clicks are harmless.
pub async fn star(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    set_starred(q, true).await
}

pub async fn unstar(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    set_starred(q, false).await
}

// Partition the request into the three favorite lists. Bare numbers count
// as tracks in `id` and as albums/artists in their own params (the
// OpenSubsonic starred examples use bare numeric ids). Returns an error
// message for the first undecodable id.
pub(crate) type IdPartition = (Vec<u64>, Vec<u64>, Vec<u64>);

pub(crate) fn partition_ids(q: &QueryParams) -> Result<IdPartition, &'static str> {
    let mut tracks = Vec::new();
    let mut albums = Vec::new();
    let mut artists = Vec::new();
    for id in &q.id.0 {
        if let Some(n) = ids::decode(ids::IdKind::Track, id).or_else(|| id.parse().ok()) {
            tracks.push(n);
        } else if let Some(n) = ids::decode(ids::IdKind::Album, id) {
            albums.push(n);
        } else if let Some(n) = ids::decode(ids::IdKind::Artist, id) {
            artists.push(n);
        } else {
            return Err("Not found");
        }
    }
    for id in &q.album_id.0 {
        let Some(n) = ids::decode(ids::IdKind::Album, id).or_else(|| id.parse().ok()) else {
            return Err("Album not found");
        };
        albums.push(n);
    }
    for id in &q.artist_id.0 {
        let Some(n) = ids::decode(ids::IdKind::Artist, id).or_else(|| id.parse().ok()) else {
            return Err("Artist not found");
        };
        artists.push(n);
    }
    Ok((tracks, albums, artists))
}

async fn apply_toggles(
    client: &crate::tidal::client::TidalClient,
    kind: FavoriteKind,
    ids: &[u64],
    starred: bool,
) -> Result<(), ()> {
    for &id in ids {
        let result = if starred {
            client.add_favorite(kind, id).await
        } else {
            client.remove_favorite(kind, id).await
        };
        if let Err(e) = result {
            tracing::error!("tidal favorite toggle failed: {e}");
            return Err(());
        }
    }
    Ok(())
}

async fn set_starred(q: QueryParams, starred: bool) -> Result<warp::reply::Json, warp::Rejection> {
    let (tracks, albums, artists) = match partition_ids(&q) {
        Ok(v) => v,
        Err(msg) => return Ok(fail(70, msg)),
    };
    let client = crate::tidal::client();
    for (kind, ids) in [
        (FavoriteKind::Track, tracks),
        (FavoriteKind::Album, albums),
        (FavoriteKind::Artist, artists),
    ] {
        if apply_toggles(client, kind, &ids, starred).await.is_err() {
            return Ok(fail(0, if starred { "Star failed" } else { "Unstar failed" }));
        }
    }
    Ok(ok(PingResponse {}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(s: &str) -> QueryParams {
        QueryParams::from_merged(s).unwrap()
    }

    #[test]
    fn partition_handles_all_id_shapes() {
        let q = params("id=t1&id=2&id=al3&id=ar4&albumId=al5&albumId=6&artistId=ar7&id=garbage");
        assert!(partition_ids(&q).is_err());

        let q = params("id=t1&id=2&id=al3&id=ar4&albumId=al5&albumId=6&artistId=ar7");
        let (tracks, albums, artists) = partition_ids(&q).unwrap();
        assert_eq!(tracks, vec![1, 2]);
        assert_eq!(albums, vec![3, 5, 6]);
        assert_eq!(artists, vec![4, 7]);
    }

    #[test]
    fn empty_request_partitions_to_empty() {
        let q = params("");
        let (tracks, albums, artists) = partition_ids(&q).unwrap();
        assert!(tracks.is_empty() && albums.is_empty() && artists.is_empty());
    }

    #[test]
    fn favorite_tracks_carry_the_favorite_time() {
        let result = serde_json::json!({
            "items": [{
                "type": "track",
                "created": "2023-01-15T10:00:00.000Z",
                "item": {
                    "id": 123,
                    "title": "Song One",
                    "duration": 220,
                    "trackNumber": 3,
                    "artists": [{"id": 9, "name": "Artist A"}],
                    "album": {"id": 456, "title": "Album One"}
                }
            }]
        });
        let songs = favorite_track_songs(&result);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].created, "2023-01-15T10:00:00.000Z");
        assert_eq!(
            songs[0].starred.as_deref(),
            Some("2023-01-15T10:00:00.000Z")
        );
        assert_eq!(
            songs[0].starred_at.as_deref(),
            Some("2023-01-15T10:00:00.000Z")
        );
    }
}

