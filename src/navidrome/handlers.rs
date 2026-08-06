use std::sync::Mutex;

use super::auth::Unauthorized;
use super::ids::{self, IdKind};
use super::models::{
    AlbumId3, AlbumList2, AlbumList2Response, AlbumWithSongs, Genres, GenresResponse,
    GetAlbumResponse, GetArtistResponse, GetOpenSubsonicExtensionsResponse, GetUserResponse,
    JukeboxControlResponse, JukeboxPlaylist, JukeboxStatus, OpenSubsonicExtension, PingResponse,
    Playlists, PlaylistsResponse, SearchResult3, SearchResult3Response, SubsonicBody, SubsonicError,
    SubsonicErrorBody, SubsonicResponse, TopSongs, TopSongsResponse, User, Child, ArtistWithAlbums,
};
use super::params::QueryParams;
use crate::SETTINGS;
use crate::tidal::mapping::{
    album_from_tidal, artist_from_tidal, artist_pic_url, cover_url, favorite_album_from_tidal,
    playlist_from_tidal, search_items, song_from_track,
};
use warp::reject::Rejection;
use warp::Reply;

fn ok<T: serde::Serialize>(data: T) -> warp::reply::Json {
    warp::reply::json(&SubsonicResponse {
        inner: SubsonicBody {
            status: "ok",
            version: "1.16.1",
            server_type: "Subtidal",
            server_version: "0.1.0",
            open_subsonic: true,
            data,
        },
    })
}


fn fail(code: u32, message: &'static str) -> warp::reply::Json {
    warp::reply::json(&SubsonicResponse {
        inner: SubsonicErrorBody {
            status: "failed",
            version: "1.16.1",
            server_type: "Subtidal",
            server_version: "0.1.0",
            open_subsonic: true,
            error: SubsonicError { code, message },
        },
    })
}

pub async fn ping() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(PingResponse {}))
}

// getOpenSubsonicExtensions: advertise the OpenSubsonic extensions the server
// supports. Mirrors Navidrome v0.63.2 (server/subsonic/opensubsonic.go).
// Public endpoint: Navidrome serves it without authentication.
pub async fn get_open_subsonic_extensions() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(GetOpenSubsonicExtensionsResponse {
        extensions: vec![
            OpenSubsonicExtension { name: "transcodeOffset", versions: vec![1] },
            OpenSubsonicExtension { name: "formPost", versions: vec![1] },
            OpenSubsonicExtension { name: "songLyrics", versions: vec![1, 2] },
            OpenSubsonicExtension { name: "indexBasedQueue", versions: vec![1] },
            OpenSubsonicExtension { name: "transcoding", versions: vec![1] },
            OpenSubsonicExtension { name: "playbackReport", versions: vec![1] },
            OpenSubsonicExtension { name: "topSongsByArtistId", versions: vec![1] },
        ],
    }))
}

// Converts middleware rejections into Subsonic error replies.
// Any other rejection propagates to the 404 fallback route.
pub async fn recover(r: Rejection) -> Result<warp::reply::Json, Rejection> {
    if r.find::<Unauthorized>().is_some() {
        Ok(fail(40, "Wrong username or password"))
    } else {
        Err(r)
    }
}

pub async fn get_user() -> Result<warp::reply::Json, warp::Rejection> {
    // The response describes the Tidal account, whatever username the
    // client passed. Fall back to the configured username when the
    // profile is unreachable.
    let settings = SETTINGS.get().expect("settings not loaded");
    let fallback = settings.username.clone();
    let (username, email) = match crate::tidal::client().user_profile().await {
        Ok(v) => {
            let name = v["username"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| v["profileName"].as_str().filter(|s| !s.is_empty()))
                .unwrap_or(&fallback)
                .to_string();
            // Tidal's username is often the login email itself.
            let email = if name.contains('@') {
                name.clone()
            } else {
                format!("{name}@localhost")
            };
            (name, email)
        }
        Err(e) => {
            tracing::warn!("user profile fetch failed: {e}");
            (fallback.clone(), format!("{fallback}@localhost"))
        }
    };
    Ok(ok(GetUserResponse {
        user: User {
            folder: vec![1],
            username,
            email,
            scrobbling_enabled: "false", // flips on when scrobble middleware is configured
            admin_role: "true",
            settings_role: "true",
            download_role: "true",
            playlist_role: "true",
            cover_art_role: "true",
            stream_role: "true",
            upload_role: "false",
            comment_role: "false",
            podcast_role: "false",
            jukebox_role: "false",
            share_role: "false",
        },
    }))
}

// getCoverArt: resolve a cover id to a Tidal image URL and 302-redirect
// there. The server never proxies image bytes. Accepted ids:
//   - a full image URL (redirect straight through)
//   - a bare UUID (playlist cover; playlists use UUID ids)
//   - al<id> / ar<id> / bare album number
pub async fn get_cover_art(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing").into_response());
    };
    let size = q.size.unwrap_or(640);
    let (uuid, artist_pic) = if id.starts_with("http") {
        (Some(id.clone()), false)
    } else if id.contains('-') {
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
    Ok(warp::reply::with_status(
        warp::reply::with_header(warp::reply(), "Location", url),
        warp::http::StatusCode::FOUND,
    )
    .into_response())
}

pub async fn search3(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(query) = q.query else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if query.trim().is_empty() {
        return Ok(ok(SearchResult3Response {
            search_result: SearchResult3 {
                artist: vec![],
                album: vec![],
                song: vec![],
            },
        }));
    }
    let result = match crate::tidal::client().search(&query).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal search failed: {e}");
            return Ok(fail(0, "Search failed"));
        }
    };
    let slice = |section: &str, count: Option<u32>, offset: Option<u32>| {
        let count = count.unwrap_or(20) as usize;
        let offset = offset.unwrap_or(0) as usize;
        search_items(&result, section)
            .into_iter()
            .skip(offset)
            .take(count)
            .collect::<Vec<_>>()
    };
    Ok(ok(SearchResult3Response {
        search_result: SearchResult3 {
            artist: slice("artists", q.artist_count, q.artist_offset)
                .into_iter()
                .filter_map(artist_from_tidal)
                .collect(),
            album: slice("albums", q.album_count, q.album_offset)
                .into_iter()
                .filter_map(album_from_tidal)
                .collect(),
            song: slice("tracks", q.song_count, q.song_offset)
                .into_iter()
                .filter_map(song_from_track)
                .collect(),
        },
    }))
}

// getAlbum: one album plus its tracks in track order. The album's year
// fills in for tracks, which carry no release date of their own.
pub async fn get_album(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    // al<id>, or a bare number as a raw Tidal album id.
    let Some(album_id) = ids::decode(IdKind::Album, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Album not found"));
    };
    let client = crate::tidal::client();
    let detail = match client.album(album_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal album fetch failed: {e}");
            return Ok(fail(0, "Album unavailable"));
        }
    };
    let tracks = match client.album_tracks(album_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal album tracks fetch failed: {e}");
            return Ok(fail(0, "Album unavailable"));
        }
    };
    let mut album = match album_from_tidal(&detail) {
        Some(a) => a,
        None => return Ok(fail(70, "Album not found")),
    };
    let year = album.year;
    let mut song: Vec<Child> = tracks["items"]
        .as_array()
        .map(|items| items.iter().filter_map(song_from_track).collect())
        .unwrap_or_default();
    for s in &mut song {
        if s.year.is_none() {
            s.year = year;
        }
    }
    Ok(ok(GetAlbumResponse {
        album: AlbumWithSongs { album, song },
    }))
}

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

// getAlbumList2: Subsonic's album listing. The mapping mirrors the
// working TidalDrome reference: starred/frequent/recent/byGenre map to
// favorites, newest maps to the personalized home feed (which includes the
// "Suggested new albums for you" section), random shuffles favorites, and
// alphabeticalByName/alphabeticalByArtist sort favorites. byYear filters
// favorites by fromYear. All favorites are fetched for the sorted and
// filtered types, then paginated locally.
pub async fn get_album_list2(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let offset = q.offset.unwrap_or(0);
    let size = q.size.unwrap_or(10).min(500);
    let album: Vec<AlbumId3> = match q.r#type.as_deref() {
        Some("starred" | "frequent" | "recent" | "byGenre") => {
            let result = match crate::tidal::client().favorite_albums(offset, size).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal favorites fetch failed: {e}");
                    return Ok(fail(0, "Album list unavailable"));
                }
            };
            favorites_albums(&result)
        }
        Some("random") => {
            let result = match crate::tidal::client().favorite_albums(offset, size).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal favorites fetch failed: {e}");
                    return Ok(fail(0, "Album list unavailable"));
                }
            };
            let mut album = favorites_albums(&result);
            shuffle(&mut album);
            album
        }
        Some("newest") => {
            let result = match crate::tidal::client().home_feed("static").await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal home feed fetch failed: {e}");
                    return Ok(fail(0, "Album list unavailable"));
                }
            };
            let raw = crate::tidal::client::albums_from_page(&result);
            raw.iter()
                .skip(offset as usize)
                .take(size as usize)
                .filter_map(album_from_tidal)
                .collect()
        }
        Some("alphabeticalByName" | "alphabeticalByArtist" | "byYear") => {
            let result = match crate::tidal::client().favorite_albums(0, 2000).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("tidal favorites fetch failed: {e}");
                    return Ok(fail(0, "Album list unavailable"));
                }
            };
            let mut album = favorites_albums(&result);
            match q.r#type.as_deref() {
                Some("alphabeticalByName") => album.sort_by(|a, b| {
                    a.name.to_lowercase().cmp(&b.name.to_lowercase())
                }),
                Some("alphabeticalByArtist") => album.sort_by(|a, b| {
                    a.artist.to_lowercase().cmp(&b.artist.to_lowercase())
                }),
                _ => {
                    let year = q.from_year.unwrap_or(0);
                    album.retain(|a| a.year == Some(year));
                }
            }
            album.into_iter().skip(offset as usize).take(size as usize).collect()
        }
        _ => Vec::new(),
    };
    Ok(ok(AlbumList2Response {
        album_list: AlbumList2 { album },
    }))
}

// Map a Tidal favorites response to AlbumID3 items.
fn favorites_albums(result: &serde_json::Value) -> Vec<AlbumId3> {
    result["items"]
        .as_array()
        .map(|items| items.iter().filter_map(favorite_album_from_tidal).collect())
        .unwrap_or_default()
}

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
    let playlist = result["items"]
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

// jukeboxControl: server-side playback state machine. There is no audio
// output, so `playing` and `position` only mirror commands; the playlist
// holds raw Tidal track ids (t<id> or bare numbers, same as stream) and
// resolves to real tracks on get. Entries that no longer exist are skipped.
static JUKEBOX: Mutex<Jukebox> = Mutex::new(Jukebox {
    playlist: Vec::new(),
    current_index: 0,
    playing: false,
    gain: 0.0,
    position: 0,
});

struct Jukebox {
    playlist: Vec<u64>,
    current_index: u32,
    playing: bool,
    gain: f32,
    position: u32,
}

// Fisher-Yates with xorshift32; the jukebox avoids extra dependencies.
fn shuffle<T>(playlist: &mut Vec<T>) {
    fn next(seed: &mut u32) -> u32 {
        let mut x = *seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *seed = x;
        x
    }
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0x9E37_79B9);
    let len = playlist.len();
    for i in (1..len).rev() {
        let j = (next(&mut seed) % (i as u32 + 1)) as usize;
        playlist.swap(i, j);
    }
}

// jukeboxControl: state changes happen under the lock; the track lookups
// run after it drops, so the mutex never spans an await.
pub async fn jukebox_control(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let action = q.action.as_deref().unwrap_or("");
    let (status, ids, with_playlist) = {
        let mut jukebox = JUKEBOX.lock().unwrap();
        match action {
            "set" => {
                jukebox.playlist = q
                    .id
                    .0
                    .iter()
                    .filter_map(|s| ids::parse_track_id(s))
                    .collect();
                if !jukebox.playlist.is_empty() {
                    jukebox.current_index =
                        jukebox.current_index.min((jukebox.playlist.len() - 1) as u32);
                }
            }
            "start" => jukebox.playing = true,
            "stop" => jukebox.playing = false,
            "skip" => {
                if let Some(i) = q.index {
                    if !jukebox.playlist.is_empty() {
                        jukebox.current_index = i.min((jukebox.playlist.len() - 1) as u32);
                    }
                }
                if let Some(pos) = q.offset {
                    jukebox.position = pos;
                }
            }
            "add" => {
                jukebox
                    .playlist
                    .extend(q.id.0.iter().filter_map(|s| ids::parse_track_id(s)));
            }
            "clear" => {
                jukebox.playlist.clear();
                jukebox.current_index = 0;
                jukebox.playing = false;
                jukebox.position = 0;
            }
            "remove" => {
                if let Some(i) = q.index {
                    let i = i as usize;
                    if i < jukebox.playlist.len() {
                        jukebox.playlist.remove(i);
                    }
                }
            }
            "shuffle" => shuffle(&mut jukebox.playlist),
            "setGain" => {
                if let Some(g) = q.gain {
                    jukebox.gain = g.clamp(0.0, 1.0);
                }
            }
            "get" | "status" => {}
            _ => return Ok(fail(0, "Unknown jukebox action")),
        }
        (
            JukeboxStatus {
                current_index: jukebox.current_index,
                playing: jukebox.playing,
                gain: jukebox.gain,
                position: jukebox.position,
            },
            jukebox.playlist.clone(),
            matches!(action, "get" | "status"),
        )
    };

    let mut entries = Vec::new();
    if with_playlist {
        let client = crate::tidal::client();
        for id in &ids {
            match client.track(*id).await {
                Ok(v) => {
                    if let Some(child) = song_from_track(&v) {
                        entries.push(child);
                    }
                }
                Err(e) => tracing::warn!("jukebox track fetch failed: {e}"),
            }
        }
    }

    Ok(ok(JukeboxControlResponse {
        status,
        playlist: with_playlist.then(|| JukeboxPlaylist { entry: entries }),
    }))
}
