// Structured lyrics: getLyricsBySongId and the legacy getLyrics. Tidal
// returns plain text plus an LRC subtitle track for the same song; the
// synced one wins when both exist. Only the version 1 shape is served:
// no kind field unless enhanced=true was requested, no cueLine data.
use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::{
    LyricLine, Lyrics, LyricsList, LyricsListResponse, LyricsResponse, StructuredLyrics,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::client::Error;
use crate::tidal::mapping::search_items;

pub async fn get_lyrics_by_song_id(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::decode(IdKind::Track, id).or_else(|| id.parse().ok()) else {
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
    let entry = if !synced.is_empty() {
        StructuredLyrics {
            display_artist,
            display_title,
            lang: "und".into(),
            offset: 0,
            synced: true,
            kind,
            line: synced,
        }
    } else {
        StructuredLyrics {
            display_artist,
            display_title,
            lang: "und".into(),
            offset: 0,
            synced: false,
            kind,
            line: unsynced,
        }
    };
    Ok(ok(LyricsListResponse {
        lyrics_list: LyricsList {
            structured_lyrics: vec![entry],
        },
    }))
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
