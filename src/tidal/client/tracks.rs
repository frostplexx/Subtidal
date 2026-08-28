// Track endpoints. The /tracks/{id} response is the source of truth for
// catalog metadata, so track() returns a typed TidalTrack instead of raw
// JSON. Songs on search/playlist/mix pages share this shape, so the same
// struct serves any Value that comes off a track-bearing endpoint.
use serde::{Deserialize, Serialize};

use serde_json::Value;

use super::{jsonapi, TidalClient};

impl TidalClient {
    // Walk the user collection items pages for one kind, flatten the
    // entries, and slice by offset/limit. Kept here so tracks/albums/
    // artists each expose the same (offset, limit) signature.
    pub(crate) async fn favorite_pages(
        &self,
        collection: &str,
        include: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Value, super::Error> {
        let mut items: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params: Vec<(&str, &str)> = vec![
                ("sort", "-addedAt"),
                ("include", include),
            ];
            if let Some(c) = &cursor {
                params.push(("page[cursor]", c.as_str()));
            }
            let doc = self
                .openapi_get(
                    &format!("/userCollection{collection}/me/relationships/items"),
                    &params,
                    &self.meta_cache,
                )
                .await?;
            let page = jsonapi::flatten_item_entries(&doc, false)["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            match jsonapi::next_cursor(&doc) {
                Some(c) => cursor = Some(c),
                None => {
                    items.extend(page);
                    break;
                }
            }
            // Walking stops a page early once the whole slice is in hand.
            items.extend(page);
            if items.len() >= offset as usize + limit as usize {
                break;
            }
        }
        let total = items.len() as u64;
        let start = offset as usize;
        let end = start + limit as usize;
        let items: Vec<Value> = items
            .into_iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect();
        Ok(serde_json::json!({
            "items": items,
            "totalNumberOfItems": total,
        }))
    }

    // Favorited tracks, newest first. Backs getStarred/getStarred2.
    // Same { items: [{ item, created }] } wrapper v1 used; pagination
    // walks page[cursor] and slices to the requested offset/limit.
    pub async fn favorite_tracks(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
        self.favorite_pages(
            "Tracks",
            "items,items.albums.coverArt,items.artists,items.genres",
            offset,
            limit,
        )
        .await
    }

    // A track's detail page. The typed TidalTrack carries everything the
    // handlers need (title, artists, duration, mixes) and serializes back
    // to raw Tidal JSON for the legacy Value-based mappers.
    pub async fn track(&self, id: u64) -> Result<TidalTrack, super::Error> {
        let value = self
            .get_json(&format!("/tracks/{id}"), &self.meta_cache)
            .await?;
        serde_json::from_value(value).map_err(super::Error::Json)
    }

    // Tidal's built-in lyrics: plain text plus an LRC subtitle track.
    // Tracks without lyrics return an empty lyrics relationship; the
    // handlers treat the resulting 404 as "no lyrics".
    pub async fn track_lyrics(&self, track_id: u64) -> Result<Value, super::Error> {
        let doc = self
            .openapi_get(
                &format!("/tracks/{track_id}"),
                &[("include", "lyrics")],
                &self.meta_cache,
            )
            .await?;
        if doc["data"]["relationships"]["lyrics"]["data"]
            .as_array()
            .is_none_or(|a| a.is_empty())
        {
            return Err(super::Error::Tidal(404, "track has no lyrics".into()));
        }
        Ok(jsonapi::flatten_resource(&doc["data"], &doc))
    }

    // --- v1 backups (dead code) ------------------------------------
    #[allow(dead_code)]
    pub async fn track_lyrics_v1(&self, track_id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/tracks/{track_id}/lyrics"), &self.meta_cache)
            .await
    }

    #[allow(dead_code)]
    pub async fn favorite_tracks_v1(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
        let user_id = self.user_id().await?;
        let offset = offset.to_string();
        let limit = limit.to_string();
        let path = format!("/users/{user_id}/favorites/tracks");
        self.get_json_q(
            &path,
            &[
                ("limit", limit.as_str()),
                ("offset", offset.as_str()),
                ("order", "DATE"),
                ("orderDirection", "DESC"),
            ],
            &self.meta_cache,
        )
        .await
    }
}

// A single Tidal track as returned by /tracks/{id}. All fields optional
// except id and title, matching the field set sone deserializes; the
// catalog can omit items (an artist with no artists list, a Master
// track missing isrc), so `#[serde(default)]` keeps deserialization
// resilient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TidalTrack {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default)]
    pub version: Option<String>,
    /// Single `artist` object, present on some endpoints.
    #[serde(default)]
    pub artist: Option<TidalArtist>,
    /// Plural `artists` array; the preferred shape on track pages.
    #[serde(default)]
    pub artists: Option<Vec<TidalArtist>>,
    #[serde(default)]
    pub album: Option<TidalAlbum>,
    #[serde(default)]
    pub audio_quality: Option<String>,
    #[serde(default)]
    pub track_number: Option<u32>,
    #[serde(default)]
    pub volume_number: Option<u32>,
    #[serde(default)]
    pub date_added: Option<String>,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub explicit: Option<bool>,
    #[serde(default)]
    pub popularity: Option<u32>,
    #[serde(default)]
    pub replay_gain: Option<f64>,
    #[serde(default)]
    pub peak: Option<f64>,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub stream_ready: Option<bool>,
    #[serde(default)]
    pub allow_streaming: Option<bool>,
    #[serde(default)]
    pub premium_streaming_only: Option<bool>,
    #[serde(default)]
    pub stream_start_date: Option<String>,
    #[serde(default)]
    pub audio_modes: Option<Vec<String>>,
    #[serde(default)]
    pub media_metadata: Option<MediaMetadata>,
    /// Mix IDs such as TRACK_MIX, present on track detail responses.
    #[serde(default)]
    pub mixes: Option<Value>,
    /// "track" or "video", from playlist item wrappers.
    #[serde(default)]
    pub item_type: Option<String>,
    /// Video thumbnail UUID; videos carry imageId instead of album.cover.
    #[serde(default)]
    pub image_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TidalArtist {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub picture: Option<String>,
    /// "MAIN" | "FEATURED" — the embedded artist role on a track.
    #[serde(default, rename = "type")]
    pub artist_type: Option<String>,
    #[serde(default)]
    pub handle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TidalAlbum {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub vibrant_color: Option<String>,
    #[serde(default)]
    pub video_cover: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
}

impl TidalTrack {
    // Rebuild raw Tidal JSON for the legacy Value-based mappers
    // (song_from_track, scrobble_song_from_track). Tidal responses only
    // hold finite floats, but guard the two float fields anyway so
    // serialize never panics on a bad value.
    pub fn to_json(&self) -> Value {
        let mut v = self.clone();
        if v.replay_gain.is_some_and(|f| !f.is_finite()) {
            v.replay_gain = None;
        }
        if v.peak.is_some_and(|f| !f.is_finite()) {
            v.peak = None;
        }
        serde_json::to_value(v).expect("TidalTrack always serializes")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaMetadata {
    #[serde(default)]
    pub tags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_deserializes_full_catalog_response() {
        let v = serde_json::json!({
            "id": 463900374,
            "title": "Song One",
            "duration": 220,
            "version": "Radio Edit",
            "artists": [{"id": 9, "name": "Artist A", "type": "MAIN"}],
            "artist": {"id": 9, "name": "Artist A"},
            "album": {"id": 456, "title": "Album One", "cover": "abc-123",
                      "releaseDate": "2021-06-25"},
            "audioQuality": "HIRES_LOSSLESS",
            "trackNumber": 3,
            "volumeNumber": 2,
            "isrc": "USYT22100001",
            "explicit": false,
            "popularity": 80,
            "replayGain": -6.2,
            "peak": 0.85,
            "copyright": "(C) 2021 Some Label",
            "url": "https://tidal.com/browse/track/463900374",
            "streamReady": true,
            "allowStreaming": true,
            "premiumStreamingOnly": false,
            "audioModes": ["STEREO"],
            "mediaMetadata": {"tags": ["LOSSLESS"]},
            "mixes": {"TRACK_MIX": "00112233445566778899aabbccddeeff"}
        });
        let t: TidalTrack = serde_json::from_value(v).unwrap();
        assert_eq!(t.id, 463900374);
        assert_eq!(t.title, "Song One");
        assert_eq!(t.duration, Some(220));
        assert_eq!(t.version.as_deref(), Some("Radio Edit"));
        assert_eq!(t.isrc.as_deref(), Some("USYT22100001"));
        assert_eq!(t.explicit, Some(false));
        assert_eq!(t.popularity, Some(80));
        assert_eq!(t.replay_gain, Some(-6.2));
        assert_eq!(t.peak, Some(0.85));
        assert_eq!(t.audio_quality.as_deref(), Some("HIRES_LOSSLESS"));
        assert_eq!(t.track_number, Some(3));
        assert_eq!(t.volume_number, Some(2));
        assert_eq!(t.mixes.as_ref().unwrap()["TRACK_MIX"], "00112233445566778899aabbccddeeff");
        let artist = t.artists.as_ref().unwrap().first().unwrap();
        assert_eq!(artist.id, 9);
        assert_eq!(artist.artist_type.as_deref(), Some("MAIN"));
        assert_eq!(artist.handle, None);
        let album = t.album.as_ref().unwrap();
        assert_eq!(album.title, "Album One");
        assert_eq!(album.release_date.as_deref(), Some("2021-06-25"));
        assert_eq!(t.media_metadata.as_ref().unwrap().tags, vec!["LOSSLESS"]);
    }

    #[test]
    fn track_accepts_sparse_object() {
        let t: TidalTrack =
            serde_json::from_value(serde_json::json!({"id": 1, "title": "X"})).unwrap();
        assert_eq!(t.duration, None);
        assert_eq!(t.isrc, None);
        assert_eq!(t.artists, None);
        assert_eq!(t.album, None);
        assert!(t.mixes.is_none());
    }
}
