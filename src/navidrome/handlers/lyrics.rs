use warp::filters::trace;

use crate::navidrome::models::song::{LyricsMode, LyricsSource, LyricsSourceNames};
// Structured lyrics: getLyricsBySongId and the legacy getLyrics. Tidal
// returns plain text plus an LRC subtitle track for the same song; the
// synced one wins when both exist. Only the version 1 shape is served:
// no kind field unless enhanced=true was requested, no cueLine data.
use super::{fail, ok};
use crate::navidrome::ids;
use crate::navidrome::models::{
    LyricLine, Lyrics, LyricsList, LyricsListResponse, LyricsResponse, StructuredLyrics,
};
use crate::navidrome::params::QueryParams;
use crate::tidal::client::Error;
use crate::tidal::mapping::search_items;

const LYRICS_SOURCES: &[LyricsSource] = &[
    LyricsSource {
        name: LyricsSourceNames::Tidal,
        endpoint: None,
        weight: 90,
    },
    LyricsSource {
        name: LyricsSourceNames::LRCLIB,
        endpoint: Some(
            "https://lrclib.net/api/get?artist_name={artist}&track_name={track}&album_name={album}&duration={duration}",
        ),
        weight: 100,
    },
    LyricsSource {
        name: LyricsSourceNames::LyricsPlus,
        endpoint: Some(
            "https://lyricsplus.prjktla.my.id/v1/lyrics/get?title={track}&artist={artist}&album={album}&duration={duration}",
        ),
        weight: 90,
    },
    LyricsSource {
        name: LyricsSourceNames::LRCMUX,
        endpoint: Some(
            "https://api.lrcmux.dev/get?artist={artist}&title={track}&album={album}&duration={duration}",
        ),
        weight: 90,
    },
];

// Track metadata that builds the source URLs and fills the Subsonic
// display fields.
#[derive(Debug)]
struct SongInfo {
    artist: String,
    title: String,
    album: String,
    duration: u32,
}

// Parsed lyrics from one source, candidate for the ranking. Not a
// Subsonic response model: the handlers translate line/plain into
// Lyrics or StructuredLyrics. line carries timed entries when mode is
// LineSynced, untimed entries otherwise.
struct Fetched {
    source: LyricsSourceNames,
    weight: u32,
    mode: LyricsMode,
    line: Vec<LyricLine>,
    plain: String,
}

// Fetch the track metadata, then every source concurrently, and order
// the results by weight. The caller picks the winner.
//
// A failing track lookup fails the whole request: without the metadata
// no source URL can be built.
async fn fetch_and_rank_lyrics(track_id: u64) -> Result<(SongInfo, Vec<Fetched>), Error> {
    let client = crate::tidal::client();
    let detail = client.track(track_id).await?;

    let song = SongInfo {
        artist: detail["artists"]
            .get(0)
            .and_then(|a| a["name"].as_str())
            .unwrap_or("")
            .to_string(),
        title: detail["title"].as_str().unwrap_or("").to_string(),
        album: detail["album"]["title"].as_str().unwrap_or("").to_string(),
        duration: detail["duration"].as_u64().unwrap_or(0) as u32,
    };

    tracing::debug!("fetch_and_rank_lyrics: {song:?}");

    let mut ranked = fetch_all(track_id, &song).await;
    // Mode dominates: word timing first, then line timing, then plain.
    // The final score breaks ties inside a mode group.
    ranked.sort_by_key(|f| std::cmp::Reverse((mode_rank(f.mode), final_score(f))));

    tracing::debug!(
        "fetch_and_rank_lyrics: ranked sources: {}",
        format_ranked(&ranked)
    );

    Ok((song, ranked))
}

fn format_ranked(ranked: &[Fetched]) -> String {
    ranked
        .iter()
        .map(|f| {
            format!(
                "{:?}@{:?}: {:?}",
                format!("{:?}", f.source),
                final_score(f),
                f.mode
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// The ranking score: the source weight plus the mode bonus.
fn final_score(f: &Fetched) -> i32 {
    f.weight as i32 + mode_bonus(f.mode)
}

// Mode dominance rank: word timing first, line timing second, plain
// last. It is the primary sort key, so a synced source always beats an
// unsynced one regardless of weight.
fn mode_rank(mode: LyricsMode) -> u8 {
    match mode {
        LyricsMode::SyllableSynced => 2,
        LyricsMode::LineSynced => 1,
        LyricsMode::Plain => 0,
    }
}

// Mode bonus: syllable timing always wins, line timing loses a little,
// plain text loses the most.
fn mode_bonus(mode: LyricsMode) -> i32 {
    match mode {
        LyricsMode::SyllableSynced => 100,
        LyricsMode::LineSynced => -50,
        LyricsMode::Plain => -100,
    }
}

// Fetch every source concurrently. A failed source contributes nothing
// to the ranking; only the metadata fetch can fail the request.
async fn fetch_all(track_id: u64, song: &SongInfo) -> Vec<Fetched> {
    futures_util::future::join_all(
        LYRICS_SOURCES
            .iter()
            .map(|source| fetch_source(source, track_id, song)),
    )
    .await
    .into_iter()
    .filter_map(|result| result.ok().flatten())
    .collect()
}

async fn fetch_source(
    source: &LyricsSource,
    track_id: u64,
    song: &SongInfo,
) -> Result<Option<Fetched>, String> {
    match source.endpoint {
        // No endpoint means Tidal: fetch the lyrics and parse the LRC subtitle track if present.
        None => {
            let lyrics = match crate::tidal::client().track_lyrics(track_id).await {
                Ok(v) => v,
                Err(Error::Tidal(404, _)) => return Ok(None),
                Err(e) => return Err(e.to_string()),
            };
            let subtitles = lyrics["subtitles"]
                .as_str()
                .map(parse_lrc)
                .unwrap_or_default();
            let text = lyrics["lyrics"].as_str().unwrap_or("");
            let line = if subtitles.is_empty() {
                parse_plain(text)
            } else {
                subtitles
            };
            Ok(Some(Fetched {
                source: source.name,
                weight: source.weight,
                mode: mode_for(&line),
                line,
                plain: text.trim().to_string(),
            }))
        }
        //TODO: Different handling for sources to extract which type of lyrics they provide (plain, line-synced, syllable-synced)
        Some(template) => {
            let url = build_url(template, song);
            let text = reqwest::get(&url)
                .await
                .map_err(|e| e.to_string())?
                .text()
                .await
                .map_err(|e| e.to_string())?;
            tracing::debug!("fetched lyrics from {:?}: {}", source.name, text);
            let synced = parse_lrc(&text);
            let line = if synced.is_empty() {
                parse_plain(&text)
            } else {
                synced
            };
            Ok(Some(Fetched {
                source: source.name,
                weight: source.weight,
                mode: mode_for(&line),
                line,
                plain: text.trim().to_string(),
            }))
        }
    }
}

// Substitute {artist}, {track}, {album} and {duration} into the source
// template. Values are percent-encoded for the query string. A single
// pass keeps a literal {track} inside an artist name intact.
fn build_url(template: &str, song: &SongInfo) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('}') else {
            panic!("unclosed placeholder in {template}");
        };
        let key = &tail[..end];
        out.push_str(&match key {
            "artist" => crate::tidal::client::percent_encode(&song.artist),
            "track" => crate::tidal::client::percent_encode(&song.title),
            "album" => crate::tidal::client::percent_encode(&song.album),
            "duration" => song.duration.to_string(),
            other => panic!("unknown placeholder {other} in {template}"),
        });
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

// Timed first entry means line-synced lyrics.
fn mode_for(line: &[LyricLine]) -> LyricsMode {
    if line.first().is_some_and(|l| l.start.is_some()) {
        LyricsMode::LineSynced
    } else {
        LyricsMode::Plain
    }
}

pub async fn get_lyrics_by_song_id(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found"));
    };
    let client = crate::tidal::client();
    // Warm the ranker and surface failures; the reply path still uses
    // the Tidal-only lookup until the ranked winner is wired in.
    if let Err(e) = fetch_and_rank_lyrics(track_id).await {
        tracing::debug!("lyrics ranker failed: {e}");
    }
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
            cue_line: None,
            agents: None,
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
            cue_line: None,
            agents: None,
        }
    };
    Ok(ok(LyricsListResponse {
        lyrics_list: LyricsList {
            structured_lyrics: vec![entry],
        },
    }))
}

// getLyrics (subsonic legacy path): lookup by artist + title, then serve the plain text.
// TODO: Update to use new ranker
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
    use super::{
        Fetched, SongInfo, build_url, final_score, mode_bonus, mode_for, mode_rank, parse_lrc,
        parse_plain, timestamp_ms,
    };
    use crate::navidrome::models::song::{LyricsMode, LyricsSourceNames};

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

    #[test]
    fn build_url_substitutes_and_encodes_placeholders() {
        let song = SongInfo {
            artist: "Borislav Slavov".into(),
            title: "I Want to Live".into(),
            album: "Baldur's Gate 3 (Original Game Soundtrack)".into(),
            duration: 233,
        };
        let url = build_url(
            "https://lrclib.net/api/get?artist_name={artist}&track_name={track}&album_name={album}&duration={duration}",
            &song,
        );
        assert_eq!(
            url,
            "https://lrclib.net/api/get?artist_name=Borislav%20Slavov&track_name=I%20Want%20to%20Live&album_name=Baldur%27s%20Gate%203%20%28Original%20Game%20Soundtrack%29&duration=233"
        );
    }

    #[test]
    fn final_score_adds_the_mode_bonus() {
        let syllable = Fetched {
            source: LyricsSourceNames::LyricsPlus,
            weight: 10,
            mode: LyricsMode::SyllableSynced,
            line: vec![],
            plain: String::new(),
        };
        let line = Fetched {
            source: LyricsSourceNames::LRCLIB,
            weight: 90,
            mode: LyricsMode::LineSynced,
            line: vec![],
            plain: String::new(),
        };
        let plain = Fetched {
            source: LyricsSourceNames::Tidal,
            weight: 100,
            mode: LyricsMode::Plain,
            line: vec![],
            plain: String::new(),
        };
        // Scores: 110, 40, 0. A light syllable source beats a heavy
        // plain one.
        assert!(final_score(&syllable) > final_score(&line));
        assert!(final_score(&line) > final_score(&plain));
        assert_eq!(mode_bonus(LyricsMode::SyllableSynced), 100);
        assert_eq!(mode_bonus(LyricsMode::LineSynced), -50);
        assert_eq!(mode_bonus(LyricsMode::Plain), -100);
    }

    #[test]
    fn ranking_keeps_declaration_order_on_equal_scores() {
        let a = Fetched {
            source: LyricsSourceNames::LRCLIB,
            weight: 100,
            mode: LyricsMode::Plain,
            line: vec![],
            plain: String::new(),
        };
        let b = Fetched {
            source: LyricsSourceNames::LyricsPlus,
            weight: 100,
            mode: LyricsMode::Plain,
            line: vec![],
            plain: String::new(),
        };
        // Both score 0; the earlier list entry survives the stable sort.
        let mut ranked = vec![b, a];
        ranked.sort_by_key(|f| std::cmp::Reverse((mode_rank(f.mode), final_score(f))));
        let sources: Vec<LyricsSourceNames> = ranked.iter().map(|f| f.source).collect();
        assert_eq!(
            sources,
            vec![LyricsSourceNames::LyricsPlus, LyricsSourceNames::LRCLIB]
        );
    }

    #[test]
    fn mode_dominates_weight_always() {
        let word = Fetched {
            source: LyricsSourceNames::Tidal,
            weight: 10,
            mode: LyricsMode::SyllableSynced,
            line: vec![],
            plain: String::new(),
        };
        let line = Fetched {
            source: LyricsSourceNames::LRCLIB,
            weight: 100,
            mode: LyricsMode::LineSynced,
            line: vec![],
            plain: String::new(),
        };
        let plain = Fetched {
            source: LyricsSourceNames::LRCMUX,
            weight: 100,
            mode: LyricsMode::Plain,
            line: vec![],
            plain: String::new(),
        };
        // The lightest synced source beats the heaviest plain one.
        let mut ranked = vec![plain, line, word];
        ranked.sort_by_key(|f| std::cmp::Reverse((mode_rank(f.mode), final_score(f))));
        let sources: Vec<LyricsSourceNames> = ranked.iter().map(|f| f.source).collect();
        assert_eq!(
            sources,
            vec![
                LyricsSourceNames::Tidal,
                LyricsSourceNames::LRCLIB,
                LyricsSourceNames::LRCMUX,
            ]
        );
    }

    #[test]
    fn mode_follows_the_first_line_timestamp() {
        assert!(matches!(mode_for(&parse_plain("hello")), LyricsMode::Plain));
        assert!(matches!(
            mode_for(&parse_lrc("[00:01.00] hello")),
            LyricsMode::LineSynced
        ));
        assert!(matches!(
            mode_for(&parse_lrc("[ar:Someone]")),
            LyricsMode::Plain
        ));
    }
}
