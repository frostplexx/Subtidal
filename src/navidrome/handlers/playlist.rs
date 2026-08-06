// Playlists and genres.
use crate::navidrome::models::{Genres, GenresResponse, Playlist, Playlists, PlaylistsResponse};
use super::{fail, ok};
use crate::tidal::mapping::playlist_from_tidal;

// getPlaylists: the user's playlists, newest first. The Subsonic API takes
// no params here; Tidal's page cap is high, so one request suffices.
pub async fn get_playlists() -> Result<warp::reply::Json, warp::Rejection> {
    let result = match crate::tidal::client().user_playlists(0, 500).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal playlists fetch failed: {e}");
            return Ok(fail(0, "Playlists unavailable"));
        }
    };
    let playlist: Vec<Playlist> = result["items"]
        .as_array()
        .map(|items| items.iter().filter_map(playlist_from_tidal).collect())
        .unwrap_or_default();
    Ok(ok(PlaylistsResponse {
        playlists: Playlists { playlist },
    }))
}

// getGenres: Tidal exposes no genre list, so the list is empty. Clients get
// a valid response; counts are unavailable until a genre source exists.
pub async fn get_genres() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(GenresResponse {
        genres: Genres { genre: vec![] },
    }))
}
