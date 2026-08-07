// Playlists and genres.
use serde_json::Value;

use crate::navidrome::models::{
    Child, Genres, GenresResponse, GetPlaylistResponse, Playlist, Playlists, PlaylistsResponse,
    PlaylistWithSongs,
};
use crate::navidrome::params::QueryParams;
use crate::tidal::client::Error;
use crate::tidal::mapping::{playlist_from_tidal, playlist_song_from_item};
use super::{fail, ok};

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

// getPlaylist: one playlist with its tracks. The id is the Tidal UUID as
// returned by getPlaylists. Items are paged at 100 (Tidal's cap for this
// endpoint); a playlist longer than 10k tracks is treated as broken.
pub async fn get_playlist(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let client = crate::tidal::client();
    let result = match client.playlist(id).await {
        Ok(v) => v,
        Err(Error::Tidal(404, _)) => return Ok(fail(70, "Playlist not found")),
        Err(e) => {
            tracing::error!("tidal playlist fetch failed: {e}");
            return Ok(fail(0, "Playlist unavailable"));
        }
    };
    let Some(playlist) = playlist_from_tidal(&result) else {
        return Ok(fail(70, "Playlist not found"));
    };
    let total = result["numberOfTracks"].as_u64().unwrap_or(0) as u32;
    let mut entry: Vec<Child> = Vec::new();
    let mut offset = 0u32;
    loop {
        let page = match client.playlist_items(id, offset, 100).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("tidal playlist items fetch failed: {e}");
                return Ok(fail(0, "Playlist unavailable"));
            }
        };
        let batch: Vec<Value> = page["items"].as_array().cloned().unwrap_or_default();
        entry.extend(batch.iter().filter_map(playlist_song_from_item));
        offset += 100;
        // Stop when all known tracks arrived, the page came back empty,
        // or the playlist is implausibly huge.
        if offset >= total || batch.is_empty() || entry.len() >= 10_000 {
            break;
        }
    }
    Ok(ok(GetPlaylistResponse {
        playlist: PlaylistWithSongs { playlist, entry },
    }))
}

// getGenres: Tidal exposes no genre list, so the list is empty. Clients get
// a valid response; counts are unavailable until a genre source exists.
pub async fn get_genres() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(GenresResponse {
        genres: Genres { genre: vec![] },
    }))
}
