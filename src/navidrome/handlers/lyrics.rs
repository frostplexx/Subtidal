use std::sync::LazyLock;
use std::time::Duration;

use crate::navidrome::models::song::{Cue, CueLine};
// Structured lyrics: getLyricsBySongId and the legacy getLyrics. Tidal
// returns plain text plus an LRC subtitle track for the same song; the
// synced one wins when both exist.
use super::{fail, fail_with, ok};
use crate::navidrome::ids;
use crate::navidrome::models::lyrics::RadiantLyrics;
use crate::navidrome::models::{
    LyricLine, Lyrics, LyricsList, LyricsListResponse, LyricsResponse, StructuredLyrics,
};
use crate::navidrome::params::QueryParams;
use crate::tidal::client::{Error, encode_query};
use crate::tidal::mapping::search_items;

// XOR-obfuscated radiant API credentials. The ciphertext and the key
// both ship in the binary; this only stops casual string scanning, same
// trick as the embedded Tidal credentials. Decoded with two_xor() below.
const ENC_ID: [u8; 8] = [0x9f, 0x99, 0x9d, 0x9f, 0xd3, 0x9d, 0x95, 0x97];
const KEY_ID: [u8; 8] = [0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae];
const ENC_TOKEN: [u8; 26] = [
    0x5a, 0x47, 0x56, 0x55, 0x2d, 0x38, 0x2a, 0x35, 0x3d, 0x22, 0x34, 0x2c, 0x3a, 0x24, 0x23, 0x20,
    0x2f, 0x7a, 0x24, 0x3c, 0x2a, 0x20, 0x64, 0x35, 0x37, 0x30,
];
const KEY_TOKEN: [u8; 26] = [
    0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b,
    0x4c, 0x4d, 0x4e, 0x4f, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55,
];
const PLATFORM: &str = "subtidal"; // always send this
const HOST: &str = "https://api.atomix.one/rl-api";

// Recover the plaintext credential bytes by XOR of cipher and key.
fn two_xor(enc: &[u8], key: &[u8]) -> String {
    enc.iter().zip(key).map(|(a, b)| (a ^ b) as char).collect()
}

static RADIANT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(RADIANT_HTTP_TIMEOUT)
        // Radiant drops idle keep-alives quickly; reusing one past that
        // fails the send. Expire pooled connections first.
        .pool_idle_timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default()
});
// Radiant's first lookup for a track can take 20s+ (it searches
// upstream before caching). The client waits RADIANT_WAIT and falls back
// to Tidal; the lookup itself keeps running up to RADIANT_HTTP_TIMEOUT
// in the background so the next request for the track hits the cache.
const RADIANT_WAIT: Duration = Duration::from_secs(60);
const RADIANT_HTTP_TIMEOUT: Duration = Duration::from_secs(90);

// Tracks with a radiant lookup in flight; a second request for the same
// track waits on the same lookup instead of starting another.
static RADIANT_IN_FLIGHT: LazyLock<std::sync::Mutex<std::collections::HashSet<u64>>> =
    LazyLock::new(Default::default);

fn radiant_client() -> &'static reqwest::Client {
    &RADIANT
}

// Radiant lookups, keyed by track: a hit, or None for a track Radiant
// has no lyrics for. Clients re-request lyrics on every play (and some
// poll), so this keeps the third-party traffic to one call per track
// per session either way.
static RADIANT_CACHE: LazyLock<moka::sync::Cache<u64, Option<StructuredLyrics>>> = LazyLock::new(|| {
    moka::sync::Cache::builder()
        .time_to_live(Duration::from_secs(6 * 3600))
        .max_capacity(5_000)
        .build()
});

async fn fetch_radiant_lyrics(track_id: u64) -> Result<StructuredLyrics, Error> {
    if let Some(hit) = RADIANT_CACHE.get(&track_id) {
        tracing::debug!("radiant lyrics cache hit for track {track_id}: found={}", hit.is_some());
        return hit.ok_or_else(|| Error::Tidal(404, "no radiant lyrics for track (cached)".into()));
    }
    // Start the lookup unless one is already running for this track.
    // It fills the cache when it finishes, whether or not anyone still
    // waits for it.
    let started = RADIANT_IN_FLIGHT.lock().map(|mut s| s.insert(track_id)).unwrap_or(false);
    if started {
        tokio::spawn(async move {
            match fetch_radiant_lyrics_uncached(track_id).await {
                Ok(lyrics) => RADIANT_CACHE.insert(track_id, Some(lyrics)),
                Err(Error::Tidal(404, msg)) => {
                    tracing::debug!("radiant lyrics: {msg}");
                    RADIANT_CACHE.insert(track_id, None);
                }
                Err(e) => tracing::warn!("radiant lyrics lookup for track {track_id} failed: {e}"),
            }
            if let Ok(mut s) = RADIANT_IN_FLIGHT.lock() {
                s.remove(&track_id);
            }
        });
    }
    // Wait for the cache to fill, up to RADIANT_WAIT.
    let deadline = tokio::time::Instant::now() + RADIANT_WAIT;
    loop {
        if let Some(hit) = RADIANT_CACHE.get(&track_id) {
            return hit.ok_or_else(|| Error::Tidal(404, "no radiant lyrics for track".into()));
        }
        let in_flight = RADIANT_IN_FLIGHT.lock().map(|s| s.contains(&track_id)).unwrap_or(false);
        if !in_flight {
            // The lookup finished without a cacheable answer (transport
            // error, bad payload); the caller falls back to Tidal.
            return Err(Error::Tidal(502, "radiant lyrics lookup failed".into()));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Tidal(504, format!(
                "radiant lyrics lookup still running after {}s; will serve from cache next time",
                RADIANT_WAIT.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn fetch_radiant_lyrics_uncached(track_id: u64) -> Result<StructuredLyrics, Error> {
    let client = crate::tidal::client();
    // Fetch the typed track; it carries title, artist, duration and isrc.
    let track = client.track(track_id).await?;

    let title = track.title.clone();
    let artist_name = track
        .artist
        .map(|a| a.name)
        .or_else(|| {
            track
                .artists
                .as_ref()
                .and_then(|list| list.first())
                .map(|a| a.name.clone())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_default();

    // Fields the typed track exposes map to the /lyrics params directly.
    let mut params = Vec::new();
    params.push(("platform", PLATFORM.to_string()));
    params.push(("title", title.clone()));
    if !artist_name.is_empty() {
        params.push(("artist", artist_name.clone()));
    }
    if let Some(isrc) = &track.isrc {
        params.push(("isrc", isrc.clone()));
    }
    if let Some(duration) = track.duration {
        params.push(("duration", duration.to_string()));
    }
    if let Some(album) = &track.album {
        params.push(("album", album.title.clone()));
    }

    let url = format!("{HOST}?{}", encode_query(&params));
    tracing::debug!("radiant lyrics lookup for track {track_id}: {url}");

    let send = || {
        radiant_client()
            .get(&url)
            .header("P-Access-Token-Id", two_xor(&ENC_ID, &KEY_ID))
            .header("P-Access-Token", two_xor(&ENC_TOKEN, &KEY_TOKEN))
            .send()
    };
    // One retry on a transport failure (a stale pooled connection is
    // the usual cause); a timeout or a response is final.
    let resp = match send().await {
        Ok(r) => r,
        Err(e) if e.is_request() && !e.is_timeout() => {
            tracing::debug!("radiant lyrics send failed for track {track_id} ({e}); retrying once");
            send().await.map_err(Error::Http)?
        }
        Err(e) => return Err(Error::Http(e)),
    };
    let status = resp.status();
    let body = resp.text().await.map_err(Error::Http)?;
    tracing::debug!("radiant lyrics response for track {track_id}: {status} ({} bytes)", body.len());
    tracing::trace!("radiant lyrics body for track {track_id}: {body}");

    if !status.is_success() {
        return Err(Error::Tidal(
            status.as_u16(),
            format!("radiant lyrics lookup failed: {body}"),
        ));
    }
    let radiant: RadiantLyrics = serde_json::from_str(&body).map_err(Error::Json)?;
    Ok(radiant_to_structured(radiant, &artist_name, &title))
}

// Map a radiant lyrics payload into a single synced structuredLyrics
// entry (enhanced v2 shape: cueLine + per-syllable cues). Each data
// line becomes a timed parent line and a cueLine; its syllabus entries
// become cues. startTime/endTime are in seconds, so they convert to the
// milliseconds the Subsonic model uses. byteStart/byteEnd are running
// UTF-8 byte offsets into the line text: each syllable advances the
// cursor by its byte length, so offsets land on the exact substring.
fn radiant_to_structured(
    radiant: RadiantLyrics,
    display_artist: &str,
    display_title: &str,
) -> StructuredLyrics {
    // type "None" is plain text: every timing is 0. Serve it unsynced
    // with no cues, or clients would stack every line at 0:00.
    if radiant.kind == "None" {
        return StructuredLyrics {
            display_artist: display_artist.to_string(),
            display_title: display_title.to_string(),
            lang: String::new(),
            offset: 0,
            synced: false,
            kind: Some("main"),
            line: radiant
                .data
                .into_iter()
                .map(|l| LyricLine { start: None, value: l.text })
                .collect(),
            cue_line: None,
            agents: None,
        };
    }
    let mut line = Vec::with_capacity(radiant.data.len());
    let mut cue_line = Vec::with_capacity(radiant.data.len());
    for (idx, l) in radiant.data.into_iter().enumerate() {
        let start_ms = (l.start_time * 1000.0) as u32;
        let end_ms = (l.end_time * 1000.0) as u32;
        line.push(LyricLine {
            start: Some(start_ms),
            value: l.text.clone(),
        });
        let mut cue = Vec::with_capacity(l.syllabus.len());
        let mut cursor = 0usize;
        // Radiant emits the odd empty syllable (a pause); it covers no
        // bytes, and would push every later offset in the line by one.
        for s in l.syllabus.into_iter().filter(|s| !s.text.is_empty()) {
            let byte_start = cursor;
            // Inclusive UTF-8 byte end: start + byte_len - 1.
            let byte_end = cursor + s.text.len().saturating_sub(1);
            cursor = byte_end.saturating_add(1);
            cue.push(Cue {
                start: s.time,
                end: Some(s.time.saturating_add(s.duration)),
                value: s.text,
                byte_start: byte_start as u32,
                byte_end: byte_end as u32,
            });
        }
        cue_line.push(CueLine {
            index: idx as u32,
            start: Some(start_ms),
            end: Some(end_ms),
            value: l.text,
            agent_id: None,
            cue,
        });
    }
    StructuredLyrics {
        display_artist: display_artist.to_string(),
        display_title: display_title.to_string(),
        lang: String::new(),
        offset: 0,
        synced: true,
        kind: Some("main"),
        line,
        // "Line" payloads carry no syllables: plain synced lines, no v2 cues.
        cue_line: cue_line.iter().any(|c| !c.cue.is_empty()).then_some(cue_line),
        agents: None,
    }
}

pub async fn get_lyrics_by_song_id(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found"));
    };
    // word_synced_lyrics routes to the third-party radiant service;
    // otherwise use Tidal's built-in lyrics.
    let word_synced = crate::SETTINGS
        .get()
        .map(|s| s.word_synced_lyrics)
        .unwrap_or(false);
    let result = if word_synced {
        match fetch_radiant_lyrics(track_id).await {
            Ok(v) if v.synced => Ok(v),
            // Plain text from radiant: Tidal's own LRC is better when
            // it exists; otherwise the plain text still beats nothing.
            Ok(plain) => match fetch_tidal_lyrics(track_id).await {
                Ok(t) if t.synced => Ok(t),
                _ => Ok(plain),
            },
            Err(e) => {
                tracing::warn!("radiant lyrics failed ({e}); falling back to tidal");
                fetch_tidal_lyrics(track_id).await
            }
        }
    } else {
        fetch_tidal_lyrics(track_id).await
    };
    let lyrics = match result {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("lyrics fetch failed: {e}");
            return Ok(fail_with(0, "Lyrics unavailable", &e));
        }
    };
    Ok(ok(LyricsListResponse {
        lyrics_list: LyricsList {
            structured_lyrics: vec![lyrics],
        },
    }))
}

// Tidal's built-in lyrics: plain text plus an LRC subtitle track. The
// synced LRC wins when both exist; otherwise the plain text is served
// as a single unsynced line.
async fn fetch_tidal_lyrics(track_id: u64) -> Result<StructuredLyrics, Error> {
    let client = crate::tidal::client();
    let value = match client.track_lyrics(track_id).await {
        Ok(v) => v,
        Err(Error::Tidal(404, _)) => {
            return Err(Error::Tidal(404, "no tidal lyrics for track".into()));
        }
        Err(e) => return Err(e),
    };
    let plain = value["lyrics"].as_str().unwrap_or("").trim();
    let subtitles = value["subtitles"].as_str().unwrap_or("");
    let mut lines = parse_lrc(subtitles);
    if !lines.is_empty() {
        Ok(StructuredLyrics {
            display_artist: String::new(),
            display_title: String::new(),
            lang: String::new(),
            offset: 0,
            synced: true,
            kind: None,
            line: std::mem::take(&mut lines),
            cue_line: None,
            agents: None,
        })
    } else {
        Ok(StructuredLyrics {
            display_artist: String::new(),
            display_title: String::new(),
            lang: String::new(),
            offset: 0,
            synced: false,
            kind: None,
            line: vec![LyricLine {
                start: None,
                value: plain.to_string(),
            }],
            cue_line: None,
            agents: None,
        })
    }
}

// getLyrics (subsonic legacy path): lookup by artist + title, then serve the plain text.
pub async fn get_lyrics(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(artist) = q.artist.as_deref() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(title) = q.title.as_deref() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let client = crate::tidal::client();
    let result = match client.search(&format!("{artist} {title}")).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal search failed: {e}");
            return Ok(fail_with(0, "Lyrics unavailable", &e));
        }
    };
    let Some(track) = search_items(&result, "tracks").into_iter().next() else {
        return Ok(ok(lyrics_reply(artist, title, "")));
    };
    let Some(track_id) = track["id"].as_u64() else {
        return Ok(ok(lyrics_reply(artist, title, "")));
    };
    let value = match client.track_lyrics(track_id).await {
        Ok(lyrics) => lyrics["lyrics"].as_str().unwrap_or("").trim().to_string(),
        Err(Error::Tidal(404, _)) => String::new(),
        Err(e) => {
            tracing::error!("tidal lyrics fetch failed: {e}");
            return Ok(fail_with(0, "Lyrics unavailable", &e));
        }
    };
    Ok(ok(lyrics_reply(artist, title, &value)))
}

fn lyrics_reply(artist: &str, title: &str, value: &str) -> LyricsResponse {
    LyricsResponse {
        lyrics: Lyrics {
            artist: artist.to_string(),
            title: title.to_string(),
            value: value.to_string(),
        },
    }
}

// Parse an LRC subtitle track into timed lines. The first timestamp on
// each line wins; metadata tags such as [ar:...] are skipped.
fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some(end) = rest.find(']') else {
            continue;
        };
        let Some(start) = timestamp_ms(&rest[..end]) else {
            continue;
        };
        let value = rest[end + 1..].trim();
        if !value.is_empty() {
            out.push(LyricLine {
                start: Some(start),
                value: value.into(),
            });
        }
    }
    out
}

// Parse mm:ss.xx into milliseconds. Returns None for metadata tags and
// anything that is not a timestamp.
fn timestamp_ms(tag: &str) -> Option<u32> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes = minutes.parse::<u32>().ok()?;
    let seconds = seconds.parse::<f64>().ok()?;
    Some(minutes * 60_000 + (seconds * 1000.0) as u32)
}

#[cfg(test)]
mod radiant_tests {
    use super::*;
    use crate::navidrome::models::lyrics::{
        RadiantLine, RadiantLyrics, RadiantMetadata, RadiantSyllable,
    };

    fn sample() -> RadiantLyrics {
        RadiantLyrics {
            kind: "Word".into(),
            data: vec![RadiantLine {
                text: "눈을 뜬 순간".into(),
                start_time: 2.747,
                duration: 3.467,
                end_time: 6.214,
                syllabus: vec![
                    RadiantSyllable {
                        text: "눈".into(),
                        time: 2747,
                        duration: 271,
                        is_background: false,
                    },
                    RadiantSyllable {
                        text: "을".into(),
                        time: 3018,
                        duration: 161,
                        is_background: false,
                    },
                    RadiantSyllable {
                        text: " ".into(),
                        time: 3179,
                        duration: 403,
                        is_background: false,
                    },
                    RadiantSyllable {
                        text: "뜬".into(),
                        time: 3582,
                        duration: 518,
                        is_background: false,
                    },
                    RadiantSyllable {
                        text: " ".into(),
                        time: 4100,
                        duration: 400,
                        is_background: false,
                    },
                    RadiantSyllable {
                        text: "순".into(),
                        time: 4500,
                        duration: 700,
                        is_background: false,
                    },
                    RadiantSyllable {
                        text: "간".into(),
                        time: 5200,
                        duration: 1014,
                        is_background: false,
                    },
                ],
                element: serde_json::json!({}),
                translation: None,
            }],
            metadata: RadiantMetadata {
                source: "Deezer".into(),
                song_writers: vec![],
                copyright: None,
                licence: None,
            },
        }
    }

    #[test]
    fn empty_syllable_does_not_shift_byte_offsets() {
        let mut r = sample();
        r.data[0].text = "a - b".into();
        r.data[0].syllabus = vec![
            RadiantSyllable { text: "a ".into(), time: 0, duration: 1, is_background: false },
            RadiantSyllable { text: "- ".into(), time: 1, duration: 1, is_background: false },
            RadiantSyllable { text: "".into(), time: 1, duration: 1, is_background: false },
            RadiantSyllable { text: "b".into(), time: 2, duration: 1, is_background: false },
        ];
        let sc = radiant_to_structured(r, "artist", "title");
        let cues = &sc.cue_line.unwrap()[0].cue;
        assert_eq!(cues.len(), 3);
        assert_eq!((cues[2].byte_start, cues[2].byte_end), (4, 4));
        assert_eq!(&"a - b"[4..=4], "b");
    }

    #[test]
    fn plain_text_payload_is_unsynced() {
        let mut plain = sample();
        plain.kind = "None".into();
        for l in &mut plain.data {
            l.start_time = 0.0;
            l.end_time = 0.0;
            l.syllabus.clear();
        }
        let sc = radiant_to_structured(plain, "artist", "title");
        assert!(!sc.synced);
        assert!(sc.cue_line.is_none());
        assert_eq!(sc.line.len(), 1);
        assert_eq!(sc.line[0].start, None);
        assert_eq!(sc.line[0].value, "눈을 뜬 순간");
    }

    #[test]
    fn maps_lines_and_cuelines() {
        let sc = radiant_to_structured(sample(), "artist", "title");
        // One parent line, timed, synced, kind main.
        assert_eq!(sc.line.len(), 1);
        assert_eq!(sc.line[0].start, Some(2747));
        assert_eq!(sc.line[0].value, "눈을 뜬 순간");
        assert!(sc.synced);
        assert_eq!(sc.kind, Some("main"));
        // One cueLine mirroring the line.
        let cl = sc.cue_line.as_ref().unwrap();
        assert_eq!(cl.len(), 1);
        assert_eq!(cl[0].index, 0);
        assert_eq!(cl[0].start, Some(2747));
        assert_eq!(cl[0].end, Some(6214));
        assert_eq!(cl[0].value, "눈을 뜬 순간");
        assert_eq!(cl[0].cue.len(), 7);
        // Agents stay unset for unattributed lyrics.
        assert!(sc.agents.is_none());
    }

    #[test]
    fn cue_byte_offsets_are_utf8_running_indices() {
        let sc = radiant_to_structured(sample(), "a", "t");
        let cl = &sc.cue_line.as_ref().unwrap()[0].cue;
        // 눈=3, 을=3, ' '=1, 뜬=3, ' '=1, 순=3, 간=3 bytes.
        let expected = [
            (0u32, 2u32),
            (3, 5),
            (6, 6),
            (7, 9),
            (10, 10),
            (11, 13),
            (14, 16),
        ];
        for (i, (bs, be)) in expected.iter().enumerate() {
            assert_eq!(cl[i].byte_start, *bs, "cue {i} byteStart");
            assert_eq!(cl[i].byte_end, *be, "cue {i} byteEnd");
        }
        // Cue times carry through; end = time + duration.
        assert_eq!(cl[0].start, 2747);
        assert_eq!(cl[0].end, Some(2747 + 271));
    }

    #[test]
    fn cue_end_present_on_every_cue() {
        let sc = radiant_to_structured(sample(), "a", "t");
        let cues = &sc.cue_line.as_ref().unwrap()[0].cue;
        assert!(cues.iter().all(|c| c.end.is_some()));
        // All-or-none end contract holds here (all have ends).
    }

    #[test]
    fn parse_lrc_extracts_timed_lines() {
        let lrc = "[ar:Artist]\n[00:13.67]Ich steppe\n[00:17.36]Packung Bifi\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start, Some(13_670));
        assert_eq!(lines[0].value, "Ich steppe");
        assert_eq!(lines[1].start, Some(17_360));
        assert_eq!(lines[1].value, "Packung Bifi");
        // Metadata tags and blank lines are skipped.
        assert!(parse_lrc("[ar:Artist]").is_empty());
    }

    #[test]
    fn xor_credentials_decode_to_plaintext() {
        assert_eq!(two_xor(&ENC_ID, &KEY_ID), "8145x189");
        assert_eq!(
            two_xor(&ENC_TOKEN, &KEY_TOKEN),
            "fzhjmyhvygrkrmikc7jszq6fce"
        );
    }
}
