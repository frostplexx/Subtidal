use crate::navidrome::models::song::{Cue, CueLine, LyricsMode, LyricsSource, LyricsSourceNames};
// Structured lyrics: getLyricsBySongId and the legacy getLyrics. Tidal
// returns plain text plus an LRC subtitle track for the same song; the
// synced one wins when both exist. Only the version 1 shape is served:
// no kind field unless enhanced=true was requested, no cueLine data.
use super::{fail, ok};
use crate::navidrome::ids;
use crate::navidrome::models::lyrics::{
    Fetched, Lrclib, Lrcmux, LrcmuxWord, LyricsPlusBody, LyricsPlusToken, SongInfo,
};
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
        LyricsMode::SyllableSynced => 3,
        LyricsMode::WordSynced => 2,
        LyricsMode::LineSynced => 1,
        LyricsMode::Plain => 0,
    }
}

// Mode bonus: syllable timing wins most, then word timing, then line
// timing; plain text loses the most. The rank is the primary sort key,
// so syncing wins regardless of these values.
fn mode_bonus(mode: LyricsMode) -> i32 {
    match mode {
        LyricsMode::SyllableSynced => 100,
        LyricsMode::WordSynced => 60,
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
                cue_line: vec![],
            }))
        }
        Some(template) => {
            let url = build_url(template, song);
            let text = reqwest::get(&url)
                .await
                .map_err(|e| e.to_string())?
                .text()
                .await
                .map_err(|e| e.to_string())?;
            Ok(normalize(source, &text))
        }
    }
}

fn normalize(source: &LyricsSource, text: &str) -> Option<Fetched> {
    let (mode, line, plain, cue_line) = match source.name {
        // Raw LRC or plain text; used by Tidal-style bodies.
        LyricsSourceNames::Tidal => {
            let line = parse_lrc(text);
            let (mode, line) = if line.is_empty() {
                (LyricsMode::Plain, parse_plain(text))
            } else {
                (LyricsMode::LineSynced, line)
            };
            (mode, line, text.trim().to_string(), vec![])
        }
        // LRCLIB: LRC lives in syncedLyrics, plain in plainLyrics. LRC
        // carries line timing only, never word or syllable cues.
        LyricsSourceNames::LRCLIB => {
            let v: Lrclib = serde_json::from_str(text).ok()?;
            let line = v
                .synced_lyrics
                .map(|s| parse_lrc(&s))
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| parse_plain(&v.plain_lyrics));
            let mode = mode_for(&line);
            (mode, line, v.plain_lyrics, vec![])
        }
        // LRCMUX: one timed line per entry. At the word level each word
        // carries its own offset, so a cueLine is emitted per line.
        LyricsSourceNames::LRCMUX => {
            let v: Lrcmux = serde_json::from_str(text).ok()?;
            let words = v.meta.level == "word";
            let mut line = Vec::new();
            let mut cue_line = Vec::new();
            for l in v.lines {
                let word_cues = if words { lrcmux_cues(&l.words) } else { None };
                let value = match &word_cues {
                    Some((cv, _)) => cv.clone(),
                    None => l.text.clone(),
                };
                if value.is_empty() {
                    continue;
                }
                let index = line.len() as u32;
                line.push(LyricLine {
                    start: Some(l.start),
                    value: value.clone(),
                });
                if let Some((_, cues)) = word_cues {
                    cue_line.push(CueLine {
                        index,
                        start: Some(l.start),
                        end: Some(l.end),
                        value,
                        agent_id: None,
                        cue: cues,
                    });
                }
            }
            let plain = line
                .iter()
                .map(|l| l.value.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let mode = if words {
                LyricsMode::WordSynced
            } else {
                LyricsMode::LineSynced
            };
            (mode, line, plain, cue_line)
        }
        // LyricsPlus: word/syllable tokens grouped into lines, with a
        // cueLine per line built from the same tokens.
        LyricsSourceNames::LyricsPlus => {
            let v: LyricsPlusBody = serde_json::from_str(text).ok()?;
            let (line, plain, cue_line) = lyrics_plus_lines(&v.lyrics);
            (LyricsMode::SyllableSynced, line, plain, cue_line)
        }
    };
    Some(Fetched {
        source: source.name,
        weight: source.weight,
        mode,
        line,
        plain,
        cue_line,
    })
}

fn lyrics_plus_lines(
    tokens: &[LyricsPlusToken],
) -> (Vec<LyricLine>, String, Vec<CueLine>) {
    let mut line = Vec::new();
    let mut plain = String::new();
    let mut cue_line = Vec::new();
    let mut group: Vec<&LyricsPlusToken> = Vec::new();
    for t in tokens {
        group.push(t);
        if t.is_line_ending == 1 {
            flush_lyrics_plus(&mut group, &mut line, &mut plain, &mut cue_line);
        }
    }
    flush_lyrics_plus(&mut group, &mut line, &mut plain, &mut cue_line);
    (line, plain, cue_line)
}

fn flush_lyrics_plus(
    group: &mut Vec<&LyricsPlusToken>,
    line: &mut Vec<LyricLine>,
    plain: &mut String,
    cue_line: &mut Vec<CueLine>,
) {
    if group.is_empty() {
        return;
    }
    let (value, cues) = token_cues(group);
    let start = group.first().map(|t| t.time);
    let end = group.last().map(|t| t.time.saturating_add(t.duration));
    if !value.is_empty() {
        let index = line.len() as u32;
        line.push(LyricLine {
            start,
            value: value.clone(),
        });
        if !cues.is_empty() {
            cue_line.push(CueLine {
                index,
                start,
                end,
                value,
                agent_id: None,
                cue: cues,
            });
        }
        plain.push_str(&line.last().unwrap().value);
        plain.push('\n');
    }
    group.clear();
}

// Build the LRCMUX line text from its words. Concatenating the raw word
// texts reproduces the renderable line exactly, which keeps byteStart /
// byteEnd offsets valid against the value we emit.
fn lrcmux_cues(words: &[LrcmuxWord]) -> Option<(String, Vec<Cue>)> {
    if words.is_empty() {
        return None;
    }
    let value: String = words.iter().map(|w| w.text.as_str()).collect();
    if value.is_empty() {
        return None;
    }
    let mut cursor = 0usize;
    let mut cues = Vec::with_capacity(words.len());
    for w in words {
        let len = w.text.len();
        if len == 0 {
            continue;
        }
        cues.push(Cue {
            start: w.start,
            end: Some(w.end),
            value: w.text.clone(),
            byte_start: cursor as u32,
            byte_end: (cursor + len - 1) as u32,
        });
        cursor += len;
    }
    Some((value, cues))
}

// Group a LyricsPlus line into syllable cues. The value is the token
// concatenation trimmed of leading and trailing whitespace; byte offsets
// are computed against that trimmed string. Interior spaces stay in the
// text and in their cue's byte range.
fn token_cues(tokens: &[&LyricsPlusToken]) -> (String, Vec<Cue>) {
    let raw: String = tokens.iter().map(|t| t.text.as_str()).collect();
    if raw.trim().is_empty() {
        return (String::new(), Vec::new());
    }
    let start_bound = raw.len() - raw.trim_start().len();
    let end_bound = raw.trim_end().len();
    let value = raw[start_bound..end_bound].to_string();
    let mut cursor = 0usize;
    let mut cues = Vec::with_capacity(tokens.len());
    for t in tokens {
        let cs = cursor.max(start_bound);
        let ce = (cursor + t.text.len()).min(end_bound);
        cursor += t.text.len();
        if ce <= cs {
            continue;
        }
        cues.push(Cue {
            start: t.time,
            end: Some(t.time.saturating_add(t.duration)),
            value: raw[cs..ce].to_string(),
            byte_start: (cs - start_bound) as u32,
            byte_end: (ce - start_bound - 1) as u32,
        });
    }
    (value, cues)
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
    let (song, mut ranked) = match fetch_and_rank_lyrics(track_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("lyrics ranker failed: {e}");
            return Ok(fail(0, "Lyrics unavailable"));
        }
    };
    let Some(winner) = ranked.first_mut() else {
        // No source produced lyrics; an empty list is a valid reply.
        return Ok(ok(LyricsListResponse {
            lyrics_list: LyricsList {
                structured_lyrics: vec![],
            },
        }));
    };
    let enhanced = q.enhanced.unwrap_or(false);
    let entry = StructuredLyrics {
        display_artist: song.artist.clone(),
        display_title: song.title.clone(),
        lang: "und".into(),
        offset: 0,
        synced: winner.mode != LyricsMode::Plain,
        kind: enhanced.then_some("main"),
        line: std::mem::take(&mut winner.line),
        cue_line: enhanced
            .then(|| std::mem::take(&mut winner.cue_line))
            .filter(|l| !l.is_empty()),
        agents: None,
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
        LyricsSource, build_url, final_score, lrcmux_cues, mode_bonus, mode_for, mode_rank,
        normalize, parse_lrc, parse_plain, timestamp_ms, token_cues,
    };
    use crate::navidrome::models::lyrics::{Fetched, LrcmuxWord, LyricsPlusToken, SongInfo};
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
            cue_line: vec![],
        };
        let line = Fetched {
            source: LyricsSourceNames::LRCLIB,
            weight: 90,
            mode: LyricsMode::LineSynced,
            line: vec![],
            plain: String::new(),
            cue_line: vec![],
        };
        let plain = Fetched {
            source: LyricsSourceNames::Tidal,
            weight: 100,
            mode: LyricsMode::Plain,
            line: vec![],
            plain: String::new(),
            cue_line: vec![],
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
            cue_line: vec![],
        };
        let b = Fetched {
            source: LyricsSourceNames::LyricsPlus,
            weight: 100,
            mode: LyricsMode::Plain,
            line: vec![],
            plain: String::new(),
            cue_line: vec![],
        };
        // Both score 0; the earlier list entry survives the stable sort.
        let mut ranked = Vec::from([b, a]);
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
            cue_line: vec![],
        };
        let line = Fetched {
            source: LyricsSourceNames::LRCLIB,
            weight: 100,
            mode: LyricsMode::LineSynced,
            line: vec![],
            plain: String::new(),
            cue_line: vec![],
        };
        let plain = Fetched {
            source: LyricsSourceNames::LRCMUX,
            weight: 100,
            mode: LyricsMode::Plain,
            line: vec![],
            plain: String::new(),
            cue_line: vec![],
        };
        // The lightest synced source beats the heaviest plain one.
        let mut ranked = Vec::from([plain, line, word]);
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

    #[test]
    fn lrcmux_word_cues_cover_the_line_exactly() {
        let words = vec![
            LrcmuxWord {
                text: "They ".into(),
                start: 5950,
                end: 6230,
            },
            LrcmuxWord {
                text: "want ".into(),
                start: 6230,
                end: 6610,
            },
            LrcmuxWord {
                text: "that ".into(),
                start: 6610,
                end: 7079,
            },
            LrcmuxWord {
                text: "yacht ".into(),
                start: 7079,
                end: 7329,
            },
            LrcmuxWord {
                text: "life".into(),
                start: 7329,
                end: 7476,
            },
        ];
        let (value, cues) = lrcmux_cues(&words).unwrap();
        assert_eq!(value, "They want that yacht life");
        assert_eq!(cues.len(), 5);
        assert_eq!(cues[4].value, "life");
        assert_eq!(cues[4].byte_start, 21);
        assert_eq!(cues[4].byte_end, 24);
        assert_eq!(cues[4].start, 7329);
        assert_eq!(cues[4].end, Some(7476));
    }

    #[test]
    fn lrcmux_word_body_is_word_synced_and_emits_cue_lines() {
        let source = LyricsSource {
            name: LyricsSourceNames::LRCMUX,
            endpoint: None,
            weight: 90,
        };
        let body = serde_json::json!({
            "meta": {"level": "word"},
            "lines": [{
                "text": "They want that yacht life",
                "start": 5950,
                "end": 7476,
                "words": [
                    {"text": "They ", "start": 5950, "end": 6230},
                    {"text": "want ", "start": 6230, "end": 6610},
                    {"text": "that ", "start": 6610, "end": 7079},
                    {"text": "yacht ", "start": 7079, "end": 7329},
                    {"text": "life", "start": 7329, "end": 7476}
                ]
            }]
        });
        let fetched = normalize(&source, &serde_json::to_string(&body).unwrap()).unwrap();
        assert_eq!(fetched.mode, LyricsMode::WordSynced);
        assert_eq!(fetched.line.len(), 1);
        assert_eq!(fetched.cue_line.len(), 1);
        assert_eq!(fetched.cue_line[0].index, 0);
        assert_eq!(fetched.cue_line[0].value, "They want that yacht life");
        assert_eq!(fetched.cue_line[0].cue.len(), 5);
    }

    #[test]
    fn lyricsplus_syllable_offsets_match_multibyte_utf8() {
        // "눈을 뜬 순간" reproduced from the spec example.
        let tokens = [
            LyricsPlusToken {
                time: 2747,
                duration: 271,
                text: "눈".into(),
                is_line_ending: 0,
            },
            LyricsPlusToken {
                time: 3018,
                duration: 161,
                text: "을".into(),
                is_line_ending: 0,
            },
            LyricsPlusToken {
                time: 3179,
                duration: 403,
                text: " ".into(),
                is_line_ending: 0,
            },
            LyricsPlusToken {
                time: 3582,
                duration: 518,
                text: "뜬".into(),
                is_line_ending: 0,
            },
            LyricsPlusToken {
                time: 4100,
                duration: 400,
                text: " ".into(),
                is_line_ending: 0,
            },
            LyricsPlusToken {
                time: 4500,
                duration: 700,
                text: "순".into(),
                is_line_ending: 0,
            },
            LyricsPlusToken {
                time: 5200,
                duration: 1014,
                text: "간".into(),
                is_line_ending: 1,
            },
        ];
        let bytes: Vec<&LyricsPlusToken> = tokens.iter().collect();
        let (value, cues) = token_cues(&bytes);
        assert_eq!(value, "눈을 뜬 순간");
        assert_eq!(cues.len(), 7);
        let ranges: Vec<(u32, u32)> = cues.iter().map(|c| (c.byte_start, c.byte_end)).collect();
        assert_eq!(
            ranges,
            vec![
                (0, 2),
                (3, 5),
                (6, 6),
                (7, 9),
                (10, 10),
                (11, 13),
                (14, 16)
            ]
        );
    }

    #[test]
    fn lyricsplus_body_is_syllable_synced() {
        let source = LyricsSource {
            name: LyricsSourceNames::LyricsPlus,
            endpoint: None,
            weight: 90,
        };
        let body = serde_json::json!({
            "type": "syllable",
            "lyrics": [
                {"time": 6062, "duration": 220, "text": "They ", "isLineEnding": 0},
                {"time": 6282, "duration": 193, "text": "want ", "isLineEnding": 0},
                {"time": 6475, "duration": 347, "text": "that ", "isLineEnding": 0},
                {"time": 6822, "duration": 654, "text": "life", "isLineEnding": 1}
            ]
        });
        let fetched = normalize(&source, &serde_json::to_string(&body).unwrap()).unwrap();
        assert_eq!(fetched.mode, LyricsMode::SyllableSynced);
        assert_eq!(fetched.cue_line.len(), 1);
        assert_eq!(fetched.cue_line[0].value, "They want that life");
        assert_eq!(fetched.cue_line[0].start, Some(6062));
        assert_eq!(fetched.line[0].start, Some(6062));
    }
}

// Live integration tests that call the real upstream lyric providers
// (LRCLIB, LRCMUX, LyricsPlus) and run the parsing pipeline over their
// responses. They need network access, so they are ignored by default
// and run with `cargo test -- --ignored`.
#[cfg(test)]
mod live {
    use super::{LYRICS_SOURCES, fetch_source, final_score, mode_rank};
    use crate::navidrome::models::lyrics::{Fetched, SongInfo};
    use crate::navidrome::models::song::{CueLine, LyricsMode, LyricsSourceNames};

    // Popular evergreen tracks, present in every provider. Timed data
    // varies by provider; the assertions validate the parser's
    // invariants against whatever comes back, not exact strings.
    fn known_songs() -> Vec<SongInfo> {
        vec![
            SongInfo {
                artist: "Queen".into(),
                title: "Bohemian Rhapsody".into(),
                album: "A Night at the Opera".into(),
                duration: 354,
            },
            SongInfo {
                artist: "Ed Sheeran".into(),
                title: "Shape of You".into(),
                album: "Divide".into(),
                duration: 235,
            },
            SongInfo {
                artist: "The Beatles".into(),
                title: "Yesterday".into(),
                album: "Help!".into(),
                duration: 126,
            },
        ]
    }

    // Fetch and parse every streamable provider for one song, skipping
    // whatever returns nothing. Tidal is excluded: its source needs a
    // logged-in Tidal client, not just a URL.
    async fn probe(song: &SongInfo) -> Vec<Fetched> {
        let mut out = Vec::new();
        for src in LYRICS_SOURCES {
            if src.name == LyricsSourceNames::Tidal {
                continue;
            }
            if let Some(f) = fetch_source(src, 0, song).await.ok().flatten() {
                out.push(f);
            }
        }
        out
    }

    // Every cue must point inside its parent line's value, measured in
    // UTF-8 bytes, and the cue texts must tile that value exactly. This
    // is the spec guarantee both parsers reproduce, for LRCMUX words and
    // LyricsPlus syllables alike.
    fn assert_cues_consistent(cue_line: &[CueLine]) {
        for cl in cue_line {
            let bytes = cl.value.len();
            for c in &cl.cue {
                assert!(
                    (c.byte_end as usize) < bytes,
                    "byteEnd {} out of range for value {:?} ({} bytes)",
                    c.byte_end,
                    cl.value,
                    bytes
                );
                assert!(
                    c.byte_start <= c.byte_end,
                    "byteStart {} > byteEnd {}",
                    c.byte_start,
                    c.byte_end
                );
                assert!(
                    c.start <= c.end.unwrap_or(c.start),
                    "cue start {} > end {:?}",
                    c.start,
                    c.end
                );
            }
            let tiled: String = cl
                .cue
                .iter()
                .map(|c| c.value.clone())
                .collect::<Vec<_>>()
                .concat();
            assert_eq!(
                tiled, cl.value,
                "cue texts must reproduce the cueLine value"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_sources_parse_into_consistent_candidates() {
        for song in known_songs() {
            let fetched = probe(&song).await;
            assert!(
                !fetched.is_empty(),
                "no provider returned lyrics for {} - {}",
                song.artist, song.title
            );
            for f in &fetched {
                assert!(
                    !f.line.is_empty(),
                    "{:?} produced no lines",
                    f.source
                );
                assert_cues_consistent(&f.cue_line);
            }
        }
    }

    // The pipeline must surface at least one word-level (LRCMUX) or
    // syllable-level (LyricsPlus) candidate across the probe songs, and
    // the handler's ranking must always put timed lyrics ahead of plain
    // text for these well-known tracks.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_pipeline_prefers_word_timed_lyrics() {
        let mut any_word_or_syllable = false;
        for song in known_songs() {
            let fetched = probe(&song).await;
            if fetched.iter().any(|f| {
                matches!(
                    f.mode,
                    LyricsMode::WordSynced | LyricsMode::SyllableSynced
                )
            }) {
                any_word_or_syllable = true;
            }
            // The handler's sort is (mode_rank desc, final_score desc);
            // max_by_key over the same token sequence picks the same
            // winner without needing Fetched: Clone.
            if let Some(winner) = fetched
                .iter()
                .max_by_key(|f| (mode_rank(f.mode), final_score(f)))
            {
                assert!(
                    winner.mode != LyricsMode::Plain,
                    "winner for {} - {} must be timed",
                    song.artist, song.title
                );
            }
        }
        assert!(
            any_word_or_syllable,
            "no probe song exposed word/syllable timing from LRCMUX or LyricsPlus"
        );
    }
}
