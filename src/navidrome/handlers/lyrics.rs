// Structured lyrics: getLyricsBySongId and the legacy getLyrics. Tidal
// returns plain text plus an LRC subtitle track for the same song; the
// synced one wins when both exist. Version 1 serves the line-level shape.
// Version 2 (enhanced=true) adds a kind field, and, when an aligner is
// configured, word-level cueLine timing and agent attribution.
use std::collections::BTreeMap;

use crate::aligner;
use crate::navidrome::ids;
use crate::navidrome::models::{
    char_range_to_byte_range, Cue, CueLine, LyricAgent, LyricLine, Lyrics, LyricsList,
    LyricsListResponse, LyricsResponse, StructuredLyrics,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::client::Error;
use crate::tidal::mapping::search_items;

pub async fn get_lyrics_by_song_id(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found"));
    };
    let client = crate::tidal::client();
    let lyrics = match client.track_lyrics(track_id).await {
        Ok(v) => v,
        Err(Error::Tidal(404, _)) => {
            // Tidal has no lyrics for this track; an empty list is a
            // valid success reply.
            return Ok(ok(LyricsListResponse {
                lyrics_list: LyricsList {
                    structured_lyrics: vec![],
                },
            }));
        }
        Err(e) => {
            tracing::error!("tidal lyrics fetch failed: {e}");
            return Ok(fail(0, "Lyrics unavailable"));
        }
    };
    let synced = lyrics["subtitles"]
        .as_str()
        .map(parse_lrc)
        .unwrap_or_default();
    let unsynced = lyrics["lyrics"]
        .as_str()
        .map(parse_plain)
        .unwrap_or_default();
    let (display_artist, display_title) = match client.track(track_id).await {
        Ok(detail) => (
            detail["artists"]
                .get(0)
                .and_then(|a| a["name"].as_str())
                .unwrap_or("")
                .to_string(),
            detail["title"].as_str().unwrap_or("").to_string(),
        ),
        Err(_) => (String::new(), String::new()),
    };
    let kind = match q.enhanced {
        Some(true) => Some("main"),
        _ => None,
    };
    // The base line-level shape. Version 2 adds word timing on top.
    let (line, synced) = if !synced.is_empty() {
        (synced, true)
    } else {
        (unsynced, false)
    };
    let entry = StructuredLyrics {
        display_artist: display_artist.clone(),
        display_title: display_title.clone(),
        lang: "und".into(),
        offset: 0,
        synced,
        kind,
        agents: None,
        cue_line: None,
        line: line.clone(),
    };
    // Version 2: try to add word-level timing when requested and an
    // aligner is available. Any failure falls back to the v1 shape.
    let entry = if q.enhanced == Some(true) && aligner::get().is_some() {
        match enhanced_cue_lines(&client, track_id).await {
            Some((agents, cue_lines)) => StructuredLyrics {
                agents: Some(agents),
                cue_line: Some(cue_lines),
                ..entry
            },
            None => entry,
        }
    } else {
        entry
    };
    Ok(ok(LyricsListResponse {
        lyrics_list: LyricsList {
            structured_lyrics: vec![entry],
        },
    }))
}

// Build version-2 word-level cueLine data for a track: fetch the FLAC
// stream URL, POST it with the lyric lines to the aligner, and map the
// returned words to UTF-8 byte offsets. Returns None on any failure so
// the caller falls back to the version-1 reply.
async fn enhanced_cue_lines(
    client: &crate::tidal::client::TidalClient,
    track_id: u64,
) -> Option<(Vec<LyricAgent>, Vec<CueLine>)> {
    let info = match client.stream_info(track_id, "LOSSLESS", "STREAM").await {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("aligner: stream_info failed for track {track_id}: {e}");
            return None;
        }
    };
    let Some(audio_url) = info.direct_url else {
        tracing::warn!("aligner: no direct FLAC URL for track {track_id}");
        return None;
    };

    // Reuse the same lyric text the handler already fetched. We refetch
    // here because the caller keeps only the parsed lines.
    let lyrics = match client.track_lyrics(track_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("aligner: lyrics refetch failed for track {track_id}: {e}");
            return None;
        }
    };
    let lines: Vec<String> = {
        let lrc = lyrics["subtitles"].as_str().map(parse_lrc_raw).unwrap_or_default();
        if !lrc.is_empty() {
            lrc
        } else {
            lyrics["lyrics"]
                .as_str()
                .map(parse_plain_raw)
                .unwrap_or_default()
        }
    };
    if lines.is_empty() {
        tracing::warn!("aligner: no lyric lines for track {track_id}");
        return None;
    }
    let language = detect_language(&lines);
    let Some(aligner) = aligner::get() else {
        tracing::warn!("aligner: aligner_url not configured");
        return None;
    };
    let aligned = match aligner
        .align(track_id, &audio_url, &language, &lines)
        .await
    {
        Some(a) => a,
        None => {
            tracing::warn!("aligner: sidecar call failed or empty for track {track_id}");
            return None;
        }
    };

    // Map aligned lines (by index) into cueLine entries. Bytes offsets
    // come from the aligner's char offsets via UTF-8 conversion.
    let mut by_index: BTreeMap<usize, &crate::aligner::AlignedLine> =
        aligned.iter().map(|l| (l.index, l)).collect();
    let mut cue_lines = Vec::new();
    let mut any = false;
    for (idx, value) in lines.iter().enumerate() {
        let Some(al) = by_index.remove(&idx) else {
            continue;
        };
        let mut cue = Vec::new();
        for w in &al.words {
            let Some((byte_start, byte_end)) =
                char_range_to_byte_range(&al.value, w.char_start, w.char_end)
            else {
                continue;
            };
            cue.push(Cue {
                byte_start,
                byte_end,
                value: w.text.clone(),
            });
        }
        if cue.is_empty() {
            continue;
        }
        any = true;
        cue_lines.push(CueLine {
            start: first_start(&al.words).unwrap_or(0),
            value: value.clone(),
            agent_id: Some(1),
            cue,
        });
    }
    if !any {
        return None;
    }
    let agents = vec![LyricAgent {
        id: 1,
        name: display_guess(),
        role: "main".into(),
        cues: None,
    }];
    Some((agents, cue_lines))
}

// Earliest start time (ms) across a line's words.
fn first_start(words: &[crate::aligner::AlignedWord]) -> Option<u32> {
    words
        .iter()
        .map(|w| (w.start_time * 1000.0) as u32)
        .min()
}

// The aligner requires a language from its 11 supported languages. A
// cheap heuristic on the lyrics text picks between CJK/others; Qwen's
// supported set is Chinese, English, Cantonese, French, German, Italian,
// Japanese, Korean, Portuguese, Russian, Spanish.
fn detect_language(lines: &[String]) -> &'static str {
    let joined = lines.join(" ");
    if joined.chars().any(|c| matches!(c, '\u{4e00}'..='\u{9fff}')) {
        "Chinese"
    } else if joined.chars().any(|c| matches!(c, '\u{3040}'..='\u{30ff}')) {
        "Japanese"
    } else if joined.chars().any(|c| matches!(c, '\u{ac00}'..='\u{d7af}')) {
        "Korean"
    } else {
        "English"
    }
}

// A generic display name for the single main agent. The real performer
// name is not reliably available here without another API call.
fn display_guess() -> String {
    crate::SETTINGS
        .get()
        .map(|s| s.username.clone())
        .unwrap_or_else(|| "Main vocalist".into())
}

// getLyrics (legacy): lookup by artist + title, then serve the plain
// text. The Tidal search's first track wins; a missing track or missing
// lyrics yields an empty value, not an error.
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
            return Ok(fail(0, "Lyrics unavailable"));
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
            return Ok(fail(0, "Lyrics unavailable"));
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

// Split plain lyrics into untimed lines.
fn parse_plain(text: &str) -> Vec<LyricLine> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|value| LyricLine {
            start: None,
            value: value.into(),
        })
        .collect()
}

// Raw (string-only) variants for the aligner path, which sends the
// lines verbatim and needs no timing.
fn parse_lrc_raw(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|raw| {
            let line = raw.trim();
            let rest = line.strip_prefix('[')?;
            let end = rest.find(']')?;
            timestamp_ms(&rest[..end])?;
            let value = rest[end + 1..].trim();
            (!value.is_empty()).then(|| value.to_string())
        })
        .collect()
}

fn parse_plain_raw(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{parse_lrc, parse_plain, timestamp_ms};

    const LRC: &str = "[00:00.42] ('Til I'm in the grave)\n\
[00:03.65] I want you to stay\n\
[ar:Machine Head]\n\
[ti:Davidian]\n";

    #[test]
    fn lrc_parses_timed_lines_and_skips_metadata() {
        let lines = parse_lrc(LRC);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start, Some(420));
        assert_eq!(lines[0].value, "('Til I'm in the grave)");
        assert_eq!(lines[1].start, Some(3650));
        assert_eq!(lines[1].value, "I want you to stay");
    }

    #[test]
    fn lrc_drops_empty_lines() {
        let lines = parse_lrc("[00:01.00] hello\n[00:02.00]  \n[00:03.00] world\n");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn plain_lyrics_untimed() {
        let lines = parse_plain("Line one\n\nLine two\n");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].start.is_none());
        assert_eq!(lines[1].value, "Line two");
    }

    #[test]
    fn timestamp_parses_minutes_and_millis() {
        assert_eq!(timestamp_ms("00:00.42"), Some(420));
        assert_eq!(timestamp_ms("03:05"), Some(185_000));
        assert_eq!(timestamp_ms("ar:Artist"), None);
        assert_eq!(timestamp_ms("ti:Title"), None);
    }
}
