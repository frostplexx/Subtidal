// Lyric-source DTOs and fetcher state for the lyrics handlers. These are
// not Subsonic response models: they parse external source payloads
// (LRCLIB, LRCMUX, LyricsPlus) or hold candidate state for the ranker.
use serde::Deserialize;

use crate::navidrome::models::song::{CueLine, LyricLine, LyricsMode, LyricsSourceNames};

// Track metadata that builds the source URLs and fills the Subsonic
// display fields.
#[derive(Debug)]
pub struct SongInfo {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: u32,
}

// Parsed lyrics from one source, candidate for the ranking. Not a
// Subsonic response model: the handlers translate line/plain into
// Lyrics or StructuredLyrics. line carries timed entries when mode is
// LineSynced, untimed entries otherwise.
pub struct Fetched {
    pub source: LyricsSourceNames,
    pub weight: u32,
    pub mode: LyricsMode,
    pub line: Vec<LyricLine>,
    pub plain: String,
    // Word/syllable timing emitted only when enhanced=true is requested.
    pub cue_line: Vec<CueLine>,
}

#[derive(Deserialize)]
pub struct Lrclib {
    #[serde(rename = "syncedLyrics")]
    pub synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics")]
    pub plain_lyrics: String,
}

#[derive(Deserialize)]
pub struct LrcmuxLine {
    pub text: String,
    pub start: u32,
    pub end: u32,
    pub words: Vec<LrcmuxWord>,
}

#[derive(Deserialize)]
pub struct LrcmuxWord {
    pub text: String,
    pub start: u32,
    pub end: u32,
}

#[derive(Deserialize)]
pub struct Lrcmux {
    pub meta: LrcmuxMeta,
    pub lines: Vec<LrcmuxLine>,
}

#[derive(Deserialize, Default)]
pub struct LrcmuxMeta {
    // "word" when lines carry per-word timing.
    #[serde(default)]
    pub level: String,
}

#[derive(Deserialize)]
pub struct LyricsPlusToken {
    pub time: u32,
    #[serde(default)]
    pub duration: u32,
    pub text: String,
    #[serde(rename = "isLineEnding")]
    pub is_line_ending: u32,
}

#[derive(Deserialize)]
pub struct LyricsPlusBody {
    pub lyrics: Vec<LyricsPlusToken>,
}
