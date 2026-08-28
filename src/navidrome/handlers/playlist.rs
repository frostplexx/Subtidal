// Playlists and genres.

use crate::navidrome::ids;
use crate::navidrome::models::{
    Child, Genre, Genres, GenresResponse, GetPlaylistResponse, PingResponse, Playlist, Playlists,
    PlaylistsResponse, PlaylistWithSongs,
};
use crate::navidrome::params::{IdList, QueryParams};
use crate::tidal::client::{Error, TidalClient};
use crate::tidal::mapping::{
    mix_from_tidal, mixes_from_page, playlist_from_tidal, playlist_song_from_item,
};
use super::{fail, ok};

// Whether getPlaylists blends in the Tidal mixes (show_mixes setting).
fn show_mixes() -> bool {
    crate::SETTINGS.get().map(|s| s.show_mixes).unwrap_or(true)
}

// getPlaylists: the user's playlists, newest first, then the Tidal mixes
// (Daily Mix, My Mix, Discovery) when show_mixes is on. The Subsonic API
// takes no params here; Tidal's page cap is high, so one request
// suffices. Mixes regenerate, so their fetch failure only drops the mix
// entries, never the whole list.
pub async fn get_playlists() -> Result<warp::reply::Json, warp::Rejection> {
    let client = crate::tidal::client();
    let mut playlist: Vec<Playlist> = match client.user_playlists(0, 500).await {
        Ok(v) => v["items"]
            .as_array()
            .map(|items| items.iter().filter_map(playlist_from_tidal).collect())
            .unwrap_or_default(),
        Err(e) => {
            tracing::error!("tidal playlists fetch failed: {e}");
            return Ok(fail(0, "Playlists unavailable"));
        }
    };
    if show_mixes() {
        match client.my_mixes().await {
            Ok(v) => {
                let mut mixes: Vec<Playlist> = mixes_from_page(&v)
                    .into_iter()
                    .filter_map(|m| mix_from_tidal(&m))
                    .collect();
                fill_mix_counts(client, &mut mixes).await;
                playlist.extend(mixes);
            }
            Err(e) => tracing::error!("tidal mixes fetch failed: {e}"),
        }
    }
    Ok(ok(PlaylistsResponse {
        playlists: Playlists { playlist },
    }))
}

// Mix entries carry no track count in the page; each count comes from
// the items endpoint's totalNumberOfItems (limit 1 is enough). The
// requests run in parallel; a failure leaves the count absent rather
// than failing the whole list.
async fn fill_mix_counts(client: &'static TidalClient, mixes: &mut [Playlist]) {
    let mut handles = Vec::with_capacity(mixes.len());
    for (i, m) in mixes.iter().enumerate() {
        let Some(mix_id) = m.id.strip_prefix("mx") else {
            continue;
        };
        let mix_id = mix_id.to_string();
        handles.push((i, tokio::spawn(async move {
            match client.mix_items(&mix_id, 0, 1).await {
                Ok(v) => v["totalNumberOfItems"].as_u64().map(|n| n as u32),
                Err(e) => {
                    tracing::debug!(mix_id = %mix_id, "tidal mix count fetch failed: {e}");
                    None
                }
            }
        })));
    }
    for (i, handle) in handles {
        if let Ok(Some(n)) = handle.await {
            mixes[i].song_count = Some(n);
        }
    }
}

// getPlaylist for a mix: the header comes from the mixes page (the items
// endpoint carries no title or cover), the songs from /mixes/{id}/items.
// The page lookup and the items fetch both hit the short mix cache.
async fn get_mix_playlist(
    client: &TidalClient,
    mix_id: &str,
) -> Result<warp::reply::Json, warp::Rejection> {
    let list = match client.my_mixes().await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal mixes fetch failed: {e}");
            return Ok(fail(0, "Mix unavailable"));
        }
    };
    let Some(mix) = mixes_from_page(&list)
        .into_iter()
        .find(|m| m["id"].as_str() == Some(mix_id))
    else {
        return Ok(fail(70, "Mix not found"));
    };
    let Some(playlist) = mix_from_tidal(&mix) else {
        return Ok(fail(70, "Mix not found"));
    };
    let result = match client.mix_items(mix_id, 0, 10_000).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal mix items fetch failed: {e}");
            return Ok(fail(0, "Mix unavailable"));
        }
    };
    let entry: Vec<Child> = result["items"]
        .as_array()
        .map(|items| items.iter().filter_map(playlist_song_from_item).collect())
        .unwrap_or_default();
    Ok(ok(GetPlaylistResponse {
        playlist: PlaylistWithSongs {
            playlist,
            entry,
        },
    }))
}

// getPlaylist: one playlist with its tracks. The id is the Tidal UUID as
// returned by getPlaylists. A playlist longer than 10k tracks is treated
// as broken and truncated.
pub async fn get_playlist(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let client = crate::tidal::client();
    // A mix id routes to the mix items endpoint; mixes are read-only.
    if let Some(mix_id) = id.strip_prefix("mx") {
        return get_mix_playlist(client, mix_id).await;
    }
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
    let entry = match playlist_entries(client, id).await {
        Ok(entry) => entry,
        Err(()) => return Ok(fail(0, "Playlist unavailable")),
    };
    Ok(ok(GetPlaylistResponse {
        playlist: PlaylistWithSongs { playlist, entry },
    }))
}

// All tracks of a playlist in one pass. The v1 items endpoint fetches
// pages concurrently, turning a long playlist's sequential cursor walk
// into a few parallel batches. A playlist longer than 10k tracks is
// treated as broken and truncated; the v2 cursor walk stays as the
// fallback when v1 fails or returns an unexpected shape.
async fn playlist_entries(client: &'static TidalClient, uuid: &str) -> Result<Vec<Child>, ()> {
    match TidalClient::playlist_items_parallel(client, uuid).await {
        Ok(entries) if entries.first().is_none_or(|e| e["item"].is_object()) => {
            return Ok(entries
                .iter()
                .take(10_000)
                .filter_map(playlist_song_from_item)
                .collect());
        }
        Ok(_) => {
            tracing::debug!("v1 playlist items shape unexpected; falling back to the v2 walk");
        }
        Err(e) => {
            tracing::debug!("v1 playlist items fetch failed ({e}); falling back to the v2 walk");
        }
    }
    let entries = match client.playlist_all_items(uuid).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal playlist items fetch failed: {e}");
            return Err(());
        }
    };
    Ok(entries
        .iter()
        .take(10_000)
        .filter_map(playlist_song_from_item)
        .collect())
}

// createPlaylist: create a playlist, or update one when playlistId is
// given. Without a playlistId, name is required and songIds fill the new
// playlist. With a playlistId, a name renames it and songIds replace the
// contents (Navidrome semantics). Returns the full playlist, per v1.14+.
pub async fn create_playlist(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let client = crate::tidal::client();
    let song_ids = match parse_song_ids(&q.song_id) {
        Ok(v) => v,
        Err(msg) => return Ok(fail(70, msg)),
    };
    let playlist_id = q.playlist_id.as_deref().filter(|s| !s.is_empty());
    match playlist_id {
        // Create mode.
        None => {
            let Some(name) = q.name.as_deref().filter(|s| !s.is_empty()) else {
                return Ok(fail(10, "Required parameter missing"));
            };
            let result = match client.create_playlist(name, None).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal playlist create failed: {e}");
                    return Ok(fail(0, "Playlist creation failed"));
                }
            };
            let Some(uuid) = result["uuid"].as_str().map(String::from) else {
                return Ok(fail(0, "Playlist creation failed"));
            };
            if !song_ids.is_empty()
                && let Err(e) = client.playlist_add_tracks(&uuid, &song_ids).await {
                    tracing::error!("tidal playlist fill failed: {e}");
                    return Ok(fail(0, "Playlist creation failed"));
                }
            playlist_response(client, &uuid).await
        }
        // Update mode: rename, then replace the contents.
        Some(pid) => {
            if let Some(name) = q.name.as_deref().filter(|s| !s.is_empty())
                && let Err(e) = client.update_playlist(pid, Some(name), None).await {
                    return Ok(mutation_error(e, "Playlist update failed"));
                }
            if !song_ids.is_empty()
                && let Err(e) = replace_songs(client, pid, &song_ids).await {
                    return Ok(mutation_error(e, "Playlist update failed"));
                }
            playlist_response(client, pid).await
        }
    }
}

// updatePlaylist: rename, add songs, and remove songs at positions.
// Removals run first: the indices refer to the playlist as the client
// sees it. Then additions append. public has no Tidal v1 setter, so it is
// accepted and ignored.
pub async fn update_playlist(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(pid) = q.playlist_id.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if pid.starts_with("mx") {
        return Ok(fail(0, "Mixes are read-only"));
    }
    let client = crate::tidal::client();
    if q.r#public.is_some() {
        tracing::debug!("playlist publicity changes are unsupported; public ignored");
    }
    let name = q.name.as_deref().filter(|s| !s.is_empty());
    let comment = q.comment.as_deref().filter(|s| !s.is_empty());
    if (name.is_some() || comment.is_some())
        && let Err(e) = client.update_playlist(pid, name, comment).await {
            return Ok(mutation_error(e, "Playlist update failed"));
        }
    // Subsonic positions of the tracks to drop, ascending and deduped.
    let mut indices: Vec<u32> = q
        .song_index_to_remove
        .0
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();
    indices.sort_unstable();
    indices.dedup();
    if !indices.is_empty() {
        // v2 deletes by item id, so the removal maps Subsonic track
        // positions to the current playlist entries first.
        let entries = match client.playlist_all_items(pid).await {
            Ok(v) => v,
            Err(e) => return Ok(mutation_error(e, "Playlist update failed")),
        };
        let mut addr: Vec<crate::tidal::client::ItemAddr> = Vec::new();
        let mut tracks_seen = 0u32;
        for e in &entries {
            if e["type"].as_str() == Some("tracks") && indices.contains(&tracks_seen) {
                if let (Some(id), Some(item_id)) = (entry_id(e), e["meta"]["itemId"].as_str())
                {
                    addr.push(("tracks".to_string(), id, item_id.to_string()));
                }
                tracks_seen += 1;
            }
        }
        if let Err(e) = client.playlist_remove_items(pid, &addr).await {
            return Ok(mutation_error(e, "Playlist update failed"));
        }
    }
    let adds = match parse_song_ids(&q.song_id_to_add) {
        Ok(v) => v,
        Err(msg) => return Ok(fail(70, msg)),
    };
    if !adds.is_empty()
        && let Err(e) = client.playlist_add_tracks(pid, &adds).await {
            return Ok(mutation_error(e, "Playlist update failed"));
        }
    Ok(ok(PingResponse {}))
}

// deletePlaylist: remove a saved playlist.
pub async fn delete_playlist(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(pid) = q.id.0.first().map(String::as_str).filter(|s| !s.is_empty()) else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if pid.starts_with("mx") {
        return Ok(fail(0, "Mixes are read-only"));
    }
    let client = crate::tidal::client();
    match client.delete_playlist(pid).await {
        Ok(()) => Ok(ok(PingResponse {})),
        Err(Error::Tidal(404, _) | Error::Tidal(403, _)) => Ok(fail(70, "Playlist not found")),
        Err(e) => {
            tracing::error!("tidal playlist delete failed: {e}");
            Ok(fail(0, "Playlist deletion failed"))
        }
    }
}

// The full playlist payload (header plus tracks) for createPlaylist.
async fn playlist_response(
    client: &'static TidalClient,
    uuid: &str,
) -> Result<warp::reply::Json, warp::Rejection> {
    let result = match client.playlist(uuid).await {
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
    let entry = match playlist_entries(client, uuid).await {
        Ok(entry) => entry,
        Err(()) => return Ok(fail(0, "Playlist unavailable")),
    };
    Ok(ok(GetPlaylistResponse {
        playlist: PlaylistWithSongs { playlist, entry },
    }))
}

// Replace a playlist's tracks with the given ones: clear, then add.
// v2 deletes by item id, so the clear collects every current entry's
// (type, id, itemId) address and drops them all in one request.
async fn replace_songs(client: &TidalClient, uuid: &str, track_ids: &[u64]) -> Result<(), Error> {
    let entries = client.playlist_all_items(uuid).await?;
    let addr: Vec<crate::tidal::client::ItemAddr> = entries
        .iter()
        .filter_map(|e| {
            Some((
                e["type"].as_str().unwrap_or("tracks").to_string(),
                entry_id(e)?,
                e["meta"]["itemId"].as_str()?.to_string(),
            ))
        })
        .collect();
    client.playlist_remove_items(uuid, &addr).await?;
    if !track_ids.is_empty() {
        client.playlist_add_tracks(uuid, track_ids).await?;
    }
    Ok(())
}

// The resource id of one playlist item entry, numeric or UUID.
fn entry_id(e: &serde_json::Value) -> Option<String> {
    e["item"]["id"]
        .as_str()
        .map(String::from)
        .or_else(|| e["item"]["id"].as_u64().map(|n| n.to_string()))
}

// Map a Tidal mutation error: 404 and 403 (missing or foreign playlist)
// -> 70, anything else -> 0.
fn mutation_error(e: Error, message: &'static str) -> warp::reply::Json {
    match e {
        Error::Tidal(404, _) | Error::Tidal(403, _) => fail(70, "Playlist not found"),
        e => {
            tracing::error!("tidal playlist mutation failed: {e}");
            fail(0, message)
        }
    }
}

// songId / songIdToAdd / id (play queue) values: t<id> or bare track
// numbers. A non-track id fails the whole request, like star does for
// its id lists.
pub(crate) fn parse_song_ids(list: &IdList) -> Result<Vec<u64>, &'static str> {
    let mut ids = Vec::with_capacity(list.0.len());
    for s in &list.0 {
        match ids::parse_track_id(s) {
            Some(n) => ids.push(n),
            None => return Err("Song not found"),
        }
    }
    Ok(ids)
}

// getGenres: Tidal's catalog carries genre names; counts come from the
// per-genre album/track endpoints. Clients validate every row strictly
// (name in `value`, numeric counts), so a fetch failure returns a valid
// empty list rather than an error.
pub async fn get_genres() -> Result<warp::reply::Json, warp::Rejection> {
    let client = crate::tidal::client();
    let genres = match crate::tidal::client::genre_list(client).await {
        Ok(rows) => rows
            .into_iter()
            .map(|r| Genre {
                name: r["name"].as_str().unwrap_or("").to_string(),
                song_count: r["songCount"].as_u64().unwrap_or(0) as u32,
                album_count: r["albumCount"].as_u64().unwrap_or(0) as u32,
            })
            .collect(),
        Err(e) => {
            tracing::warn!("tidal genre catalog fetch failed: {e}");
            vec![]
        }
    };
    Ok(ok(GenresResponse {
        genres: Genres { genre: genres },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> IdList {
        IdList(list.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn song_ids_accept_tracks_and_bare_numbers() {
        assert_eq!(parse_song_ids(&ids(&["t1", "2", "t30"])).unwrap(), vec![1, 2, 30]);
        assert_eq!(parse_song_ids(&ids(&[])).unwrap(), Vec::<u64>::new());
        assert_eq!(parse_song_ids(&ids(&["t1", "al2"])), Err("Song not found"));
        assert_eq!(parse_song_ids(&ids(&["junk"])), Err("Song not found"));
    }
}
