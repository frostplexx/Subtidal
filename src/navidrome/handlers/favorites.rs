// Favorites: getStarred / getStarred2. Both read the same three Tidal
// favorites lists; the difference is only the wrapping. getStarred uses
// legacy shapes (isDir albums, minimal artists); getStarred2 the ID3 shapes.
use crate::navidrome::models::{
    Child, Starred, Starred2, Starred2Album, Starred2Artist, Starred2Response, StarredAlbum,
    StarredResponse,
};
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

// Map a favorites track list to Child, carrying the favorite time.
fn favorites_songs(result: &serde_json::Value) -> Vec<Child> {
    result["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let mut song = song_from_track(&entry["item"])?;
                    song.starred = entry["created"].as_str().map(String::from);
                    Some(song)
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_favorites() -> Result<(serde_json::Value, serde_json::Value, serde_json::Value), ()> {
    let client = crate::tidal::client();
    let albums = client.favorite_albums(0, 2000);
    let artists = client.favorite_artists(0, 2000);
    let tracks = client.favorite_tracks(0, 2000);
    let (albums, artists, tracks) = match tokio::try_join!(albums, artists, tracks) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal favorites fetch failed: {e}");
            return Err(());
        }
    };
    Ok((albums, artists, tracks))
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
            song: favorites_songs(&tracks),
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
                    let artist = artist_from_tidal(&entry["item"])?;
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
            song: favorites_songs(&tracks),
        },
    }))
}
