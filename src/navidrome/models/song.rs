// Song models: the Child entry and its list wrappers.
use serde::Serialize;

// Subsonic child (song entry). Optional fields omitted when unknown.
// The OpenSubsonic schema marks contentType, suffix, size, path, created
// and isVideo as required; Feishin's song normalizer dereferences
// contentType directly, so omitting it crashes the album view.
#[derive(Serialize)]
pub struct Child {
    pub id: String,
    #[serde(rename = "parent")]
    pub parent: String,
    #[serde(rename = "isDir")]
    pub is_dir: bool,
    #[serde(rename = "isVideo")]
    pub is_video: bool,
    pub title: String,
    pub album: String,
    pub artist: String,
    #[serde(rename = "track")]
    pub track: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(rename = "coverArt", skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    pub duration: u32,
    #[serde(rename = "discNumber", skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u32>,
    #[serde(rename = "albumId")]
    pub album_id: String,
    #[serde(rename = "artistId")]
    pub artist_id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    // Placeholder media metadata: no transcode or file probe yet.
    #[serde(rename = "contentType")]
    pub content_type: &'static str,
    pub suffix: &'static str,
    pub size: u64,
    pub path: String,
    pub created: String,
    // Favorite time; present only on favorites-derived song lists. The
    // legacy `starred` is deprecated in OpenSubsonic 1.16.5, which adds
    // `starredAt`; clients read one field or the other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
    #[serde(rename = "starredAt", skip_serializing_if = "Option::is_none")]
    pub starred_at: Option<String>,
    // OpenSubsonic ReplayGain; always present, inner fields omitted when
    // unknown (Navidrome convention).
    #[serde(rename = "replayGain")]
    pub replay_gain: ReplayGain,
}

// OpenSubsonic ReplayGain: track gain in dB, track peak a positive
// amplitude. Tidal supplies both on the track object; album gain lives on
// playbackinfo only, so it is not sent. Empty fields are omitted.
#[derive(Serialize, Default)]
pub struct ReplayGain {
    #[serde(rename = "trackGain", skip_serializing_if = "Option::is_none")]
    pub track_gain: Option<f64>,
    #[serde(rename = "trackPeak", skip_serializing_if = "Option::is_none")]
    pub track_peak: Option<f64>,
}

// getSong data: { song: Child }
#[derive(Serialize)]
pub struct GetSongResponse {
    pub song: Child,
}

// getTopSongs data: { topSongs: { song: [ Child ] } }
#[derive(Serialize)]
pub struct TopSongsResponse {
    #[serde(rename = "topSongs")]
    pub top_songs: TopSongs,
}

#[derive(Serialize)]
pub struct TopSongs {
    pub song: Vec<Child>,
}

// getRandomSongs data: { randomSongs: { song: [ Child ] } }
#[derive(Serialize)]
pub struct RandomSongsResponse {
    #[serde(rename = "randomSongs")]
    pub random_songs: RandomSongs,
}

#[derive(Serialize)]
pub struct RandomSongs {
    pub song: Vec<Child>,
}

// getNowPlaying data: { nowPlaying: { entry: [ NowPlayingEntry ] } }. The
// entry is a full song plus the playback report fields.
#[derive(Serialize)]
pub struct NowPlayingResponse {
    #[serde(rename = "nowPlaying")]
    pub now_playing: NowPlaying,
}

#[derive(Serialize)]
pub struct NowPlaying {
    pub entry: Vec<NowPlayingEntry>,
}

#[derive(Serialize)]
pub struct NowPlayingEntry {
    #[serde(flatten)]
    pub song: Child,
    pub username: String,
    #[serde(rename = "minutesAgo")]
    pub minutes_ago: u32,
    #[serde(rename = "playerId")]
    pub player_id: u32,
    // playbackReport extension fields; the server always estimates a
    // position from the latest report.
    pub state: &'static str,
    #[serde(rename = "positionMs")]
    pub position_ms: u64,
    #[serde(rename = "playbackRate")]
    pub playback_rate: f64,
}

// getSimilarSongs2 data: { similarSongs2: { song: [ Child ] } }
#[derive(Serialize)]
pub struct SimilarSongs2Response {
    #[serde(rename = "similarSongs2")]
    pub similar_songs2: SimilarSongs2,
}

#[derive(Serialize)]
pub struct SimilarSongs2 {
    pub song: Vec<Child>,
}

// getLyricsBySongId data: { lyricsList: { structuredLyrics: [...] } }.
// Version 1 shape: kind is omitted unless enhanced=true was requested.
#[derive(Serialize)]
pub struct LyricsListResponse {
    #[serde(rename = "lyricsList")]
    pub lyrics_list: LyricsList,
}

#[derive(Serialize)]
pub struct LyricsList {
    #[serde(rename = "structuredLyrics")]
    pub structured_lyrics: Vec<StructuredLyrics>,
}

// getSimilarSongs v1 data: { similarSongs: { song: [ Child ] } }
#[derive(Serialize)]
pub struct SimilarSongsResponse {
    #[serde(rename = "similarSongs")]
    pub similar_songs: SimilarSongs,
}

#[derive(Serialize)]
pub struct SimilarSongs {
    pub song: Vec<Child>,
}

// getSongsByGenre data: { songsByGenre: { song: [ Child ] } }
#[derive(Serialize)]
pub struct SongsByGenreResponse {
    #[serde(rename = "songsByGenre")]
    pub songs_by_genre: SongsByGenre,
}

#[derive(Serialize)]
pub struct SongsByGenre {
    pub song: Vec<Child>,
}

// getLyrics (legacy) data: { lyrics: { artist, title, value } }. The
// plain text of the song's lyrics goes in value.
#[derive(Serialize)]
pub struct LyricsResponse {
    pub lyrics: Lyrics,
}

#[derive(Serialize)]
pub struct Lyrics {
    pub artist: String,
    pub title: String,
    pub value: String,
}


pub struct LyricsSource {
    pub name: LyricsSourceNames,
    pub endpoint: Option<&'static str>,
    pub weight: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LyricsMode {
    Plain,
    LineSynced,
    WordSynced,
    SyllableSynced,
}

#[derive(Clone, Copy, Debug, PartialEq)]
// LRCLIB and LRCMUX are provider brand names; keep their acronym casing.
#[allow(clippy::upper_case_acronyms)]
pub enum LyricsSourceNames {
    Tidal,
    LRCLIB,
    LyricsPlus,
    LRCMUX,
}

#[derive(Serialize)]
pub struct StructuredLyrics {
    #[serde(rename = "displayArtist")]
    pub display_artist: String,
    #[serde(rename = "displayTitle")]
    pub display_title: String,
    pub lang: String,
    pub offset: i32,
    pub synced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    pub line: Vec<LyricLine>,
    // OpenSubsonic songLyrics v2 fields. Gated behind enhanced=true:
    // without them the reply is identical to version 1.
    #[serde(rename = "cueLine", skip_serializing_if = "Option::is_none")]
    pub cue_line: Option<Vec<CueLine>>,
    #[serde(rename = "agents", skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<Agent>>,
}

#[derive(Serialize)]
pub struct LyricLine {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    pub value: String,
}

// OpenSubsonic songLyrics v2: word/syllable timings for one parent
// line. index points at the parent entry in structuredLyrics.line.
// agentId references an entry in structuredLyrics.agents; simple
// unattributed lyrics omit it. byteStart and byteEnd are 0-based
// inclusive offsets into the UTF-8 encoding of value.
#[derive(Serialize)]
pub struct CueLine {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u32>,
    pub value: String,
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub cue: Vec<Cue>,
}

// One timed word or syllable. start/end are milliseconds; byteStart and
// byteEnd are 0-based inclusive UTF-8 offsets into cueLine.value. end
// is present on every cue or none, per the contract.
#[derive(Serialize)]
pub struct Cue {
    pub start: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u32>,
    pub value: String,
    #[serde(rename = "byteStart")]
    pub byte_start: u32,
    #[serde(rename = "byteEnd")]
    pub byte_end: u32,
}

// The semantic vocal layer of an agent: lead/default vocals, an
// explicit individual voice part, background vocals, or chorus. Agent
// emission is not wired up yet, so the role variants are intentionally
// unused for now.
#[derive(Serialize)]
#[allow(dead_code)]
pub enum AgentRole {
    #[serde(rename = "main")]
    Main,
    #[serde(rename = "voice")]
    Voice,
    #[serde(rename = "bg")]
    Bg,
    #[serde(rename = "group")]
    Group,
}

// A reusable vocal agent within one structuredLyrics entry. id is only
// meaningful inside that entry. An attributed entry must mark exactly
// one agent as Main.
#[derive(Serialize)]
pub struct Agent {
    pub id: String,
    pub role: AgentRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn top_songs_wraps_song_array() {
        let resp = TopSongsResponse {
            top_songs: TopSongs { song: vec![] },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r##"{"topSongs":{"song":[]}}"##);
    }

    #[test]
    fn child_always_emits_replay_gain_object() {
        let song = crate::tidal::mapping::song_from_track(&json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        }))
        .unwrap();
        let json = serde_json::to_value(&song).unwrap();
        assert_eq!(json["replayGain"], json!({}));
    }

    #[test]
    fn child_replay_gain_carries_values() {
        let song = crate::tidal::mapping::song_from_track(&json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"},
            "replayGain": -8.5,
            "peak": 0.999
        }))
        .unwrap();
        let json = serde_json::to_value(&song).unwrap();
        assert_eq!(json["replayGain"]["trackGain"], json!(-8.5));
        assert_eq!(json["replayGain"]["trackPeak"], json!(0.999));
        assert!(json["replayGain"].get("albumGain").is_none());
    }

    #[test]
    fn get_song_wraps_child() {
        let song = crate::tidal::mapping::song_from_track(&json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        }))
        .unwrap();
        let json = serde_json::to_string(&GetSongResponse { song }).unwrap();
        assert!(json.contains(r#""song":{"id":"t123""#));
    }

    #[test]
    fn now_playing_entry_flattens_song() {
        let song = crate::tidal::mapping::song_from_track(&json!({
            "id": 123,
            "title": "Song One",
            "duration": 220,
            "trackNumber": 3,
            "artists": [{"id": 9, "name": "Artist A"}],
            "album": {"id": 456, "title": "Album One"}
        }))
        .unwrap();
        let json = serde_json::to_value(&NowPlayingEntry {
            song,
            username: "admin".into(),
            minutes_ago: 0,
            player_id: 0,
            state: "playing",
            position_ms: 120_000,
            playback_rate: 1.0,
        })
        .unwrap();
        assert_eq!(json["id"], "t123");
        assert_eq!(json["username"], "admin");
        assert_eq!(json["minutesAgo"], 0);
        assert_eq!(json["playerId"], 0);
        assert_eq!(json["state"], "playing");
        assert_eq!(json["positionMs"], 120_000);
        assert_eq!(json["playbackRate"], 1.0);
    }

    #[test]
    fn structured_lyrics_omits_kind_without_enhanced() {
        let json = serde_json::to_value(&StructuredLyrics {
            display_artist: "Muse".into(),
            display_title: "Hysteria".into(),
            lang: "eng".into(),
            offset: 0,
            synced: true,
            kind: None,
            line: vec![LyricLine {
                start: Some(0),
                value: "It's bugging me".into(),
            }],
            cue_line: None,
            agents: None,
        })
        .unwrap();
        assert!(json.get("kind").is_none());
        assert!(json.get("cueLine").is_none());
        assert!(json.get("agents").is_none());
        assert_eq!(json["line"][0]["start"], 0);
        assert_eq!(json["line"][0]["value"], "It's bugging me");
    }

    #[test]
    fn structured_lyrics_emits_v2_fields_with_enhanced() {
        let json = serde_json::to_value(&StructuredLyrics {
            display_artist: "Muse".into(),
            display_title: "Hysteria".into(),
            lang: "eng".into(),
            offset: 0,
            synced: true,
            kind: Some("main"),
            line: vec![LyricLine {
                start: Some(0),
                value: "It's bugging me".into(),
            }],
            cue_line: Some(vec![CueLine {
                index: 0,
                start: Some(0),
                end: Some(900),
                value: "It's".into(),
                agent_id: Some("lead".into()),
                cue: vec![Cue {
                    start: 0,
                    end: Some(400),
                    value: "It's".into(),
                    byte_start: 0,
                    byte_end: 3,
                }],
            }]),
            agents: Some(vec![Agent {
                id: "lead".into(),
                role: AgentRole::Main,
                name: Some("Matthew Bellamy".into()),
            }]),
        })
        .unwrap();
        assert_eq!(json["kind"], "main");
        assert_eq!(json["cueLine"][0]["index"], 0);
        assert_eq!(json["cueLine"][0]["start"], 0);
        assert_eq!(json["cueLine"][0]["end"], 900);
        assert_eq!(json["cueLine"][0]["cue"][0]["start"], 0);
        assert_eq!(json["cueLine"][0]["cue"][0]["end"], 400);
        assert_eq!(json["cueLine"][0]["cue"][0]["value"], "It's");
        assert_eq!(json["cueLine"][0]["cue"][0]["byteStart"], 0);
        assert_eq!(json["cueLine"][0]["cue"][0]["byteEnd"], 3);
        assert_eq!(json["cueLine"][0]["value"], "It's");
        assert_eq!(json["cueLine"][0]["agentId"], "lead");
        assert_eq!(json["agents"][0]["id"], "lead");
        assert_eq!(json["agents"][0]["role"], "main");
        assert_eq!(json["agents"][0]["name"], "Matthew Bellamy");
    }
}
