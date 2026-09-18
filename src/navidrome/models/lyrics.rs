// Lyric-source DTOs and fetcher state for the lyrics handlers. These are
// not Subsonic response models: they parse external source payloads
// (LRCLIB, LRCMUX, LyricsPlus, radiant) or hold candidate state for the
// ranker.
use serde::Deserialize;

// The radiant /lyrics response. `type` is "Word" for word-synced
// lyrics, "Line" for line-synced ones (no syllabus, no element) and
// "None" for plain text (every timing 0). Each data entry is
// one parent line; its syllabus array holds the per-word timings.
//
// Most fields are deserialization-contract data the handlers don't
// consume yet, so dead_code is allowed: removing them would silently
// drop fields the third-party API returns.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct RadiantLyrics {
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Vec<RadiantLine>,
    pub metadata: RadiantMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct RadiantLine {
    pub text: String,
    /// Seconds. The syllabus timings below are milliseconds.
    pub start_time: f64,
    pub duration: f64,
    pub end_time: f64,
    /// Absent when the line is only line-synced, not word-by-word.
    #[serde(default)]
    pub syllabus: Vec<RadiantSyllable>,
    /// Free-form object; may hold per-line extras. Raw so extra fields
    /// never break deserialization; absent on "Line" payloads.
    #[serde(default)]
    pub element: serde_json::Value,
    #[serde(default)]
    pub translation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct RadiantSyllable {
    pub text: String,
    /// Milliseconds from track start.
    pub time: u32,
    pub duration: u32,
    #[serde(default)]
    pub is_background: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct RadiantMetadata {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub song_writers: Vec<String>,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub licence: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radiant_lyrics_deserializes_word_payload() {
        let v = r##"{
            "type": "Word",
            "data": [
                {
                    "text": "Ich steppe in den Wald",
                    "startTime": 13.675,
                    "duration": 3.325,
                    "endTime": 17,
                    "syllabus": [
                        {"text": "Ich ", "time": 13675, "duration": 62, "isBackground": false},
                        {"text": "steppe ", "time": 13787, "duration": 250, "isBackground": false}
                    ],
                    "element": {},
                    "translation": null
                }
            ],
            "metadata": {
                "source": "Deezer",
                "songWriters": ["Lukas Strobel"],
                "copyright": "Sony/ATV Music Publishing LLC",
                "licence": "Lyrics Licensed & Provided by LyricFind"
            },
            "_cached": true
        }"##;
        let r: RadiantLyrics = serde_json::from_str(v).unwrap();
        assert_eq!(r.kind, "Word");
        assert_eq!(r.data.len(), 1);
        let line = &r.data[0];
        assert_eq!(line.text, "Ich steppe in den Wald");
        assert_eq!(line.start_time, 13.675);
        assert_eq!(line.duration, 3.325);
        assert_eq!(line.end_time, 17.0);
        assert_eq!(line.syllabus.len(), 2);
        assert_eq!(line.syllabus[0].text, "Ich ");
        assert_eq!(line.syllabus[0].time, 13675);
        assert!(!line.syllabus[0].is_background);
        assert_eq!(line.translation, None);
        assert_eq!(r.metadata.source, "Deezer");
        assert_eq!(r.metadata.song_writers, vec!["Lukas Strobel"]);
        assert_eq!(r.metadata.licence.as_deref(), Some("Lyrics Licensed & Provided by LyricFind"));
    }

    #[test]
    fn radiant_lyrics_accepts_missing_syllabus() {
        // Line-synced-only responses omit `syllabus` entirely rather than
        // sending an empty array; this must still parse.
        let v = r##"{
            "type": "Line",
            "data": [
                {"text": "X", "startTime": 0.0, "duration": 1.0,
                 "endTime": 1.0, "element": {}, "translation": null}
            ],
            "metadata": {"source": "Deezer"}
        }"##;
        let r: RadiantLyrics = serde_json::from_str(v).unwrap();
        assert!(r.data[0].syllabus.is_empty());
    }

    #[test]
    fn radiant_lyrics_accepts_line_payload_without_element() {
        let json = r#"{"type":"Line","data":[{"startTime":27.14,"text":"The victorious reign!","endTime":40.84,"duration":13.7}],"metadata":{"title":"The Victorious Reign","artist":"Hate Eternal","album":"I Monarch","duration":217,"source":"LRCLIB"}}"#;
        let parsed: RadiantLyrics = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.kind, "Line");
        assert_eq!(parsed.data.len(), 1);
        assert!(parsed.data[0].syllabus.is_empty());
        assert!(parsed.data[0].element.is_null());
    }

    #[test]
    fn radiant_lyrics_accepts_sparse_metadata() {
        let v = r##"{
            "type": "Word",
            "data": [{"text": "X", "startTime": 0.0, "duration": 1.0,
                       "endTime": 1.0, "syllabus": [], "element": {},
                       "translation": null}],
            "metadata": {"source": "Deezer"}
        }"##;
        let r: RadiantLyrics = serde_json::from_str(v).unwrap();
        assert!(r.metadata.song_writers.is_empty());
        assert_eq!(r.metadata.copyright, None);
    }
}


