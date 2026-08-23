// Query params shared by /rest/* endpoints. All optional at parse time;
// per-endpoint and auth requirements are enforced in handlers/auth.
//
// serde_urlencoded cannot represent repeated keys (id, albumId, artistId)
// through a derived struct — it rejects the second occurrence with
// "duplicate field". QueryParams therefore deserializes manually: every
// key/value pair is collected in wire order and assigned, repeats kept.
use serde::Deserialize;

// One or more `id` params, in wire order.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct IdList(pub Vec<String>);

#[derive(Debug, Default, Clone)]
pub struct QueryParams {
    pub u: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub p: Option<String>,
    #[allow(dead_code)]
    pub v: Option<String>,
    #[allow(dead_code)]
    pub c: Option<String>,
    // search3
    pub query: Option<String>,
    pub artist_count: Option<u32>,
    pub artist_offset: Option<u32>,
    pub album_count: Option<u32>,
    pub album_offset: Option<u32>,
    pub song_count: Option<u32>,
    pub song_offset: Option<u32>,
    // getCoverArt (single id) / jukeboxControl and star/unstar (many ids);
    // id may carry t<id>, al<id>, ar<id>, or a bare number.
    pub id: IdList,
    pub album_id: IdList,
    pub artist_id: IdList,
    // getTopSongs: artist name, or the artist id via the topSongsByArtistId
    // extension; count defaults to 50
    pub artist: Option<String>,
    // getLyrics (legacy): song lookup by artist + title
    pub title: Option<String>,
    pub count: Option<u32>,
    // jukeboxControl
    pub action: Option<String>,
    pub index: Option<u32>,
    pub gain: Option<f32>,
    // getAlbumList2
    pub r#type: Option<String>,
    pub offset: Option<u32>,
    // image/page size: getCoverArt defaults 640, getAlbumList2 defaults 20
    pub size: Option<u32>,
    // getAlbumList2 byYear
    pub from_year: Option<u32>,
    #[allow(dead_code)]
    pub to_year: Option<u32>,
    // stream: Tidal has no real transcoding, so maxBitRate picks the
    // quality tier; format is only a hint (flac lifts to LOSSLESS).
    pub max_bit_rate: Option<u32>,
    pub format: Option<String>,
    // scrobble: playback report; time is ms since epoch, submission is
    // true for a real scrobble, false for a now-playing notification.
    pub time: Option<i64>,
    pub submission: Option<bool>,
    // getRandomSongs: genre filter; year window reuses fromYear/toYear
    pub genre: Option<String>,
    // getLyricsBySongId: enhanced=true asks for the v2 shape (kind,
    // cueLine). We serve only v1, so the flag is accepted and ignored
    // beyond the shape of the reply.
    pub enhanced: Option<bool>,
    // reportPlayback: playback timeline events.
    pub media_id: Option<String>,
    pub media_type: Option<String>,
    pub position_ms: Option<u64>,
    pub state: Option<String>,
    pub playback_rate: Option<f64>,
    pub ignore_scrobble: Option<bool>,
    // setRating: 0 removes, 1-5 sets.
    pub rating: Option<u32>,
    // getIndexes/getArtists: only folder 1 exists; ifModifiedSince is
    // accepted and ignored.
    pub music_folder_id: Option<u32>,
    pub if_modified_since: Option<i64>,
    // playlist CRUD: createPlaylist (playlistId + name + songId),
    // updatePlaylist (playlistId + name/comment/public + songIdToAdd +
    // songIndexToRemove), deletePlaylist (id). Publicity has no Tidal v1
    // setter, so public is accepted and ignored.
    pub playlist_id: Option<String>,
    pub name: Option<String>,
    pub comment: Option<String>,
    pub r#public: Option<bool>,
    pub song_id: IdList,
    pub song_id_to_add: IdList,
    pub song_index_to_remove: IdList,
    // savePlayQueue: the id list is the queue (repeated id), current is
    // the playing song, position is ms within it. deleteBookmark reuses
    // id; createBookmark reuses comment. savePlayQueueByIndex uses
    // currentIndex, the queue position of the playing song.
    pub current: Option<String>,
    pub current_index: Option<i64>,
    pub position: Option<u64>,
}

impl<'de> Deserialize<'de> for QueryParams {
    fn deserialize<D: serde::de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Builder(QueryParams);
        impl<'de> serde::de::Visitor<'de> for Builder {
            type Value = QueryParams;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a urlencoded query string")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                mut self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                while let Some((k, v)) = map.next_entry::<String, String>()? {
                    assign(&mut self.0, &k, v)?;
                }
                Ok(self.0)
            }
        }
        d.deserialize_map(Builder(QueryParams::default()))
    }
}

// Assign one pair to the matching field. Unknown params are ignored, so
// new client fields cannot break the server. Numeric fields reject
// non-numeric values, mirroring serde's derived behavior.
fn assign<E: serde::de::Error>(q: &mut QueryParams, k: &str, v: String) -> Result<(), E> {
    match k {
        "u" => q.u = Some(v),
        "t" => q.t = Some(v),
        "s" => q.s = Some(v),
        "p" => q.p = Some(v),
        "v" => q.v = Some(v),
        "c" => q.c = Some(v),
        "query" => q.query = Some(v),
        "artistCount" => q.artist_count = Some(v.parse().map_err(|_| E::custom("invalid artistCount"))?),
        "artistOffset" => q.artist_offset = Some(v.parse().map_err(|_| E::custom("invalid artistOffset"))?),
        "albumCount" => q.album_count = Some(v.parse().map_err(|_| E::custom("invalid albumCount"))?),
        "albumOffset" => q.album_offset = Some(v.parse().map_err(|_| E::custom("invalid albumOffset"))?),
        "songCount" => q.song_count = Some(v.parse().map_err(|_| E::custom("invalid songCount"))?),
        "songOffset" => q.song_offset = Some(v.parse().map_err(|_| E::custom("invalid songOffset"))?),
        "id" => q.id.0.push(v),
        "albumId" => q.album_id.0.push(v),
        "artistId" => q.artist_id.0.push(v),
        "artist" => q.artist = Some(v),
        "title" => q.title = Some(v),
        "count" => q.count = Some(v.parse().map_err(|_| E::custom("invalid count"))?),
        "action" => q.action = Some(v),
        "index" => q.index = Some(v.parse().map_err(|_| E::custom("invalid index"))?),
        "gain" => q.gain = Some(v.parse().map_err(|_| E::custom("invalid gain"))?),
        "type" => q.r#type = Some(v),
        "offset" => q.offset = Some(v.parse().map_err(|_| E::custom("invalid offset"))?),
        "size" => q.size = Some(v.parse().map_err(|_| E::custom("invalid size"))?),
        "fromYear" => q.from_year = Some(v.parse().map_err(|_| E::custom("invalid fromYear"))?),
        "toYear" => q.to_year = Some(v.parse().map_err(|_| E::custom("invalid toYear"))?),
        "maxBitRate" => q.max_bit_rate = Some(v.parse().map_err(|_| E::custom("invalid maxBitRate"))?),
        "format" => q.format = Some(v),
        "time" => q.time = Some(v.parse().map_err(|_| E::custom("invalid time"))?),
        "submission" => q.submission = Some(v.parse().map_err(|_| E::custom("invalid submission"))?),
        "genre" => q.genre = Some(v),
        "enhanced" => q.enhanced = Some(v.parse().map_err(|_| E::custom("invalid enhanced"))?),
        "mediaId" => q.media_id = Some(v),
        "mediaType" => q.media_type = Some(v),
        "positionMs" => q.position_ms = Some(v.parse().map_err(|_| E::custom("invalid positionMs"))?),
        "state" => q.state = Some(v),
        "playbackRate" => q.playback_rate = Some(v.parse().map_err(|_| E::custom("invalid playbackRate"))?),
        "ignoreScrobble" => q.ignore_scrobble = Some(v.parse().map_err(|_| E::custom("invalid ignoreScrobble"))?),
        "rating" => q.rating = Some(v.parse().map_err(|_| E::custom("invalid rating"))?),
        "musicFolderId" => q.music_folder_id = Some(v.parse().map_err(|_| E::custom("invalid musicFolderId"))?),
        "ifModifiedSince" => q.if_modified_since = Some(v.parse().map_err(|_| E::custom("invalid ifModifiedSince"))?),
        "playlistId" => q.playlist_id = Some(v),
        "name" => q.name = Some(v),
        "comment" => q.comment = Some(v),
        "public" => q.r#public = Some(v.parse().map_err(|_| E::custom("invalid public"))?),
        "songId" => q.song_id.0.push(v),
        "songIdToAdd" => q.song_id_to_add.0.push(v),
        "songIndexToRemove" => q.song_index_to_remove.0.push(v),
        "current" => q.current = Some(v),
        "currentIndex" => {
            q.current_index = Some(v.parse().map_err(|_| E::custom("invalid currentIndex"))?)
        }
        "position" => q.position = Some(v.parse().map_err(|_| E::custom("invalid position"))?),
        _ => {}
    }
    Ok(())
}

impl QueryParams {
    // Merge the URL query string and a form-encoded body, then parse.
    // Subsonic clients send params either in the URL (GET) or in the
    // body (POST, the OpenSubsonic formPost extension), rarely both.
    pub fn merge_raw(query: &str, body: &[u8]) -> String {
        let body = std::str::from_utf8(body).unwrap_or("");
        match (query.is_empty(), body.is_empty()) {
            (_, true) => query.to_owned(),
            (true, false) => body.to_owned(),
            (false, false) => format!("{query}&{body}"),
        }
    }

    pub fn from_merged(merged: &str) -> Result<Self, serde_urlencoded::de::Error> {
        serde_urlencoded::from_str(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(query: &str, body: &[u8]) -> Result<QueryParams, serde_urlencoded::de::Error> {
        QueryParams::from_merged(&QueryParams::merge_raw(query, body))
    }

    #[test]
    fn parses_query_string() {
        let p = parse("u=admin&v=1.16.1&c=curl", b"").unwrap();
        assert_eq!(p.u.as_deref(), Some("admin"));
        assert_eq!(p.v.as_deref(), Some("1.16.1"));
    }

    #[test]
    fn parses_form_body() {
        let p = parse("", b"u=admin&v=1.16.1&c=curl").unwrap();
        assert_eq!(p.u.as_deref(), Some("admin"));
    }

    #[test]
    fn merges_query_and_body() {
        let p = parse("u=admin", b"query=abc").unwrap();
        assert_eq!(p.u.as_deref(), Some("admin"));
        assert_eq!(p.query.as_deref(), Some("abc"));
    }

    #[test]
    fn empty_input_yields_empty_params() {
        let p = parse("", b"").unwrap();
        assert!(p.u.is_none() && p.query.is_none());
    }

    #[test]
    fn rejects_type_mismatch() {
        // size is u32; a non-numeric value must fail to parse.
        assert!(parse("size=abc", b"").is_err());
    }

    #[test]
    fn parses_single_id_value() {
        // getCoverArt sends id as a single scalar; it must deserialize into
        // the IdList field, not fail the whole request.
        let p = parse("id=xyz", b"").unwrap();
        assert_eq!(p.id, IdList(vec!["xyz".to_string()]));
    }

    #[test]
    fn parses_repeated_id_values() {
        // star/unstar and jukeboxControl send several id params.
        let p = parse("id=t1&id=t2&id=al3", b"").unwrap();
        assert_eq!(
            p.id,
            IdList(vec!["t1".to_string(), "t2".to_string(), "al3".to_string()])
        );
        let p = parse("id=t1&albumId=al5&albumId=6&artistId=ar7", b"").unwrap();
        assert_eq!(p.album_id, IdList(vec!["al5".to_string(), "6".to_string()]));
        assert_eq!(p.artist_id, IdList(vec!["ar7".to_string()]));
    }

    #[test]
    fn parses_scrobble_params() {
        let p = parse("id=t1&submission=true&time=1786116785370", b"").unwrap();
        assert_eq!(p.submission, Some(true));
        assert_eq!(p.time, Some(1786116785370));
    }

    use proptest::prelude::*;

    // Untrusted client input drives this parser, so it must never crash.
    // Any input must produce a value or an error, never a panic.
    proptest! {
        #[test]
        fn from_merged_never_panics(p in ".*") {
            let _ = QueryParams::from_merged(&p);
        }

        #[test]
        fn merge_raw_never_panics(
            query in ".*",
            body in proptest::collection::vec(any::<u8>(), 0..256)
        ) {
            let merged = QueryParams::merge_raw(&query, &body);
            let _ = QueryParams::from_merged(&merged);
        }
    }
}
