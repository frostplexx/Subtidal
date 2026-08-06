// jukeboxControl: server-side playback state machine. There is no audio
// output, so `playing` and `position` only mirror commands; the playlist
// holds raw Tidal track ids (t<id> or bare numbers, same as stream) and
// resolves to real tracks on get. Entries that no longer exist are skipped.
use std::sync::Mutex;

use crate::navidrome::ids;
use crate::navidrome::models::{JukeboxControlResponse, JukeboxPlaylist, JukeboxStatus};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::song_from_track;

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
// getAlbumList2 type=random also reuses it to shuffle favorites.
pub(crate) fn shuffle<T>(playlist: &mut Vec<T>) {
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
