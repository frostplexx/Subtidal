// Map Tidal playlist JSON to Subsonic Playlist.
use serde_json::Value;

use crate::navidrome::models::Playlist;

use super::{cover_url, song::song_from_track};

// Tidal playlist ids are UUIDs; Subsonic keeps them as opaque strings.
// squareImage is a cover UUID; coverArt carries the full image URL so
// clients that accept URLs skip getCoverArt entirely. The v2 attributes
// use accessType/numberOfTrackItems/createdAt/lastModifiedAt and an ISO
// 8601 duration string; the older names stay as fallbacks for drift.
pub fn playlist_from_tidal(v: &Value) -> Option<Playlist> {
    let id = v["uuid"].as_str()?.to_string();
    let name = v["title"].as_str()?.to_string();
    let song_count = v["numberOfTrackItems"]
        .as_u64()
        .or_else(|| v["numberOfItems"].as_u64())
        .or_else(|| v["numberOfTracks"].as_u64())
        .map(|n| n as u32);
    let known_tracks = song_count.unwrap_or(0) > 0;
    Some(Playlist {
        id,
        name,
        comment: v["description"].as_str().map(String::from),
        owner: v["creator"]["name"].as_str().map(String::from),
        r#public: v["accessType"].as_str() == Some("PUBLIC")
            || v["publicPlaylist"].as_bool().unwrap_or(false),
        song_count,
        // A missing duration plus any count makes clients divide by
        // nothing and show NaN; 0 keeps the row renderable.
        duration: v["duration"]
            .as_str()
            .and_then(iso_duration_seconds)
            .or_else(|| v["duration"].as_u64().map(|n| n as u32))
            .or_else(|| known_tracks.then_some(0)),
        created: v["createdAt"]
            .as_str()
            .or_else(|| v["created"].as_str())
            .map(String::from),
        changed: v["lastModifiedAt"]
            .as_str()
            .or_else(|| v["lastUpdated"].as_str())
            .map(String::from),
        cover_art: v["squareImage"]
            .as_str()
            .or_else(|| v["image"].as_str())
            .map(|c| cover_url(c, 320)),
    })
}

// One entry of a playlist: the item wraps the track as { item: { ... } }.
pub fn playlist_song_from_item(v: &Value) -> Option<crate::navidrome::models::Child> {
    song_from_track(&v["item"])
}

// Collect the mix objects from a my_collection_my_mixes page: rows ->
// modules -> pagedList -> items, keeping only entries that carry a
// mixType. Video mixes ("My Video Mix"/"My Video Mixes") are dropped:
// they mix music videos, which stream differently and are not playable
// here. Any structural drift simply yields no mixes.
pub fn mixes_from_page(v: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(rows) = v["rows"].as_array() {
        for row in rows {
            if let Some(modules) = row["modules"].as_array() {
                for module in modules {
                    if module["type"].as_str() == Some("MIX_LIST")
                        && let Some(items) = module["pagedList"]["items"].as_array()
                    {
                        out.extend(
                            items
                                .iter()
                                .filter(|i| i["mixType"].is_string())
                                .filter(|i| {
                                    !i["title"]
                                        .as_str()
                                        .is_some_and(|t| t.starts_with("My Video Mix"))
                                })
                                .cloned(),
                        );
                    }
                }
            }
        }
    }
    out
}

// v2 playlist durations are ISO 8601 strings such as "P30M5S" (Tidal
// omits the T separator and means M as minutes, not months) or
// "PT1H2M3S". Seconds only; a malformed string yields None.
fn iso_duration_seconds(s: &str) -> Option<u32> {
    let mut secs: u64 = 0;
    let mut num: u64 = 0;
    for ch in s.strip_prefix('P')?.chars() {
        match ch {
            '0'..='9' => num = num * 10 + u64::from(ch as u8 - b'0'),
            // T is the (optional) date/time separator; ignore it.
            'T' => num = 0,
            'D' | 'H' | 'M' | 'S' => {
                secs += num
                    * match ch {
                        'D' => 86400,
                        'H' => 3600,
                        'M' => 60,
                        _ => 1,
                    };
                num = 0;
            }
            _ => return None,
        }
    }
    u32::try_from(secs).ok()
}

// Map a mix object to a Subsonic Playlist. The id keeps an mx prefix so
// getPlaylist can route it to the mix items endpoint; the cover is a full
// image URL, like playlist covers. Mixes have no owner or stable track
// count; created/duration get epoch/zero placeholders so clients render
// them cleanly instead of showing a broken value.
pub fn mix_from_tidal(v: &Value) -> Option<Playlist> {
    let id = format!("mx{}", v["id"].as_str()?);
    let name = v["title"].as_str()?.to_string();
    Some(Playlist {
        id,
        name,
        comment: v["subTitle"]
            .as_str()
            .or_else(|| v["description"].as_str())
            .map(String::from),
        owner: Some("TIDAL".into()),
        r#public: false,
        song_count: None,
        duration: Some(0),
        created: Some("1970-01-01T00:00:00.000Z".into()),
        changed: None,
        cover_art: v["images"]["MEDIUM"]["url"]
            .as_str()
            .or_else(|| v["images"]["SMALL"]["url"].as_str())
            .map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn playlist_item_unwraps_track() {
        let item = json!({
            "type": "track",
            "item": {
                "id": 123,
                "title": "Song One",
                "duration": 220,
                "trackNumber": 1,
                "volumeNumber": 1,
                "artists": [{"id": 9, "name": "Artist A"}],
                "album": {"id": 456, "title": "Album One", "cover": "abc-123"},
                "releaseDate": "2021-06-25"
            }
        });
        let song = playlist_song_from_item(&item).unwrap();
        assert_eq!(song.id, "t123");
        assert_eq!(song.album_id, "al456");
        assert_eq!(song.artist, "Artist A");
    }

    #[test]
    fn playlist_maps_fields() {
        let p = json!({
            "uuid": "0f31-6c0a",
            "title": "Morning Drive",
            "description": "Upbeat",
            "creator": {"id": 1, "name": "Ada"},
            "accessType": "PUBLIC",
            "numberOfTrackItems": 42,
            "duration": "P30M5S",
            "createdAt": "2023-01-15T10:00:00.000Z",
            "lastModifiedAt": "2024-02-01T08:30:00.000Z",
            "squareImage": "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"
        });
        let pl = playlist_from_tidal(&p).unwrap();
        assert_eq!(pl.id, "0f31-6c0a");
        assert_eq!(pl.owner.as_deref(), Some("Ada"));
        assert_eq!(pl.song_count, Some(42));
        // "P30M5S" is 30 minutes 5 seconds, not months.
        assert_eq!(pl.duration, Some(1805));
        assert!(pl.r#public);
        assert_eq!(pl.created.as_deref(), Some("2023-01-15T10:00:00.000Z"));
        assert_eq!(pl.changed.as_deref(), Some("2024-02-01T08:30:00.000Z"));
        assert_eq!(
            pl.cover_art.as_deref(),
            Some("https://resources.tidal.com/images/1a2b3c4d/5e6f/4a7b/8c9d/0e1f2a3b4c5d/320x320.jpg")
        );
    }

    #[test]
    fn playlist_duration_parses_iso_8601_variants() {
        assert_eq!(iso_duration_seconds("PT1H2M3S"), Some(3723));
        assert_eq!(iso_duration_seconds("P1DT2H"), Some(93600));
        assert_eq!(iso_duration_seconds("P45S"), Some(45));
        assert_eq!(iso_duration_seconds("garbage"), None);
        // Numeric legacy fallback stays intact.
        let p = json!({"uuid": "u1", "title": "T", "duration": 9134});
        assert_eq!(playlist_from_tidal(&p).unwrap().duration, Some(9134));
        // Missing duration with a known count renders 0, never NaN.
        let p = json!({"uuid": "u2", "title": "T2", "numberOfTrackItems": 7});
        assert_eq!(playlist_from_tidal(&p).unwrap().duration, Some(0));
    }



    #[test]
    fn mixes_from_page_unwraps_paged_list() {
        // Shape captured live from pages/my_collection_my_mixes.
        let page = json!({
            "id": "page-1",
            "rows": [{
                "modules": [{
                    "type": "MIX_LIST",
                    "pagedList": {
                        "totalNumberOfItems": 17,
                        "items": [
                            {"id": "mix-1", "title": "My Daily Discovery", "mixType": "DISCOVERY_MIX"},
                            {"id": "mix-2", "title": "My Mix 1", "mixType": "DAILY_MIX"},
                            {"id": "mix-3", "title": "My Video Mix", "mixType": "VIDEO_MIX"},
                            {"id": "mix-4", "title": "My Video Mix 1", "mixType": "VIDEO_MIX"}
                        ]
                    }
                }]
            }]
        });
        let mixes = mixes_from_page(&page);
        assert_eq!(mixes.len(), 2);
        assert_eq!(mixes[0]["id"], "mix-1");
        assert_eq!(mixes[1]["title"], "My Mix 1");
        assert!(mixes.iter().all(|m| !m["title"]
            .as_str()
            .is_some_and(|t| t.starts_with("My Video Mix"))));
    }

    #[test]
    fn mixes_from_page_skips_non_mix_modules() {
        let page = json!({
            "rows": [{
                "modules": [
                    {"type": "CAROUSEL", "pagedList": {"items": [{"id": "x", "mixType": "DAILY_MIX"}]}},
                    {"type": "MIX_LIST", "pagedList": {"items": [{"id": "y", "title": "T", "mixType": "DAILY_MIX"}]}}
                ]
            }]
        });
        let mixes = mixes_from_page(&page);
        assert_eq!(mixes.len(), 1);
        assert_eq!(mixes[0]["id"], "y");
    }

    #[test]
    fn mix_maps_to_prefixed_playlist() {
        let mix = json!({
            "id": "002a925a8721401af44e9ccb59a2fb",
            "title": "My Mix 1",
            "subTitle": "Cattle Decapitation, Bolt Thrower and more",
            "mixType": "DAILY_MIX",
            "images": {
                "SMALL": {"url": "https://images.tidal.com/s.jpg"},
                "MEDIUM": {"url": "https://images.tidal.com/m.jpg"}
            }
        });
        let p = mix_from_tidal(&mix).unwrap();
        assert_eq!(p.id, "mx002a925a8721401af44e9ccb59a2fb");
        assert_eq!(p.name, "My Mix 1");
        assert_eq!(p.comment.as_deref(), Some("Cattle Decapitation, Bolt Thrower and more"));
        assert_eq!(p.owner.as_deref(), Some("TIDAL"));
        assert_eq!(p.cover_art.as_deref(), Some("https://images.tidal.com/m.jpg"));
        // Placeholders so clients render cleanly instead of a broken value.
        assert_eq!(p.duration, Some(0));
        assert_eq!(p.created.as_deref(), Some("1970-01-01T00:00:00.000Z"));
    }

    #[test]
    fn mix_cover_falls_back_to_small() {
        let mix = json!({
            "id": "abc",
            "title": "T",
            "mixType": "DAILY_MIX",
            "images": {"SMALL": {"url": "https://images.tidal.com/s.jpg"}}
        });
        assert_eq!(
            mix_from_tidal(&mix).unwrap().cover_art.as_deref(),
            Some("https://images.tidal.com/s.jpg")
        );
    }
}
