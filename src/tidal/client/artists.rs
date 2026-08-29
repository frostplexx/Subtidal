// Artist endpoints. v1 bodies stay as dead-code backups under `_v1`
// names where a v2 shape exists. The album list uses v1: three
// requests cover the whole catalog, while the v2 relationship walk
// paginates 20 items at a time. Album dedup and the compilation
// stamping live on at the bottom.
use std::collections::hash_map::Entry;
use std::collections::HashMap;

use serde_json::Value;

use super::{jsonapi, TidalClient};

const ARTIST_INCLUDE: &str = "profileArt";

impl TidalClient {
    pub async fn artist(&self, id: u64) -> Result<Value, super::Error> {
        let doc = self
            .openapi_get(
                &format!("/artists/{id}"),
                &[("include", ARTIST_INCLUDE)],
                &self.meta_cache,
            )
            .await?;
        Ok(jsonapi::flatten_resource(&doc["data"], &doc))
    }

    // One artist's releases: albums, then EPs and singles, then
    // compilations. The v1 albums endpoint pages up to 1000 items and
    // splits by section, so the whole list costs three requests; the
    // v2 relationship walk would cost one cursor page per 20 albums
    // (twelve round trips for Lady Gaga). The v1 index is partial:
    // region-variant releases drop out of the list, but their direct
    // album pages still resolve through the getAlbum v2 fallback.
    pub async fn artist_albums(&self, artist_id: u64) -> Result<Value, super::Error> {
        let mut albums = self
            .get_json_q(
                &format!("/artists/{artist_id}/albums"),
                &[("limit", "1000"), ("offset", "0")],
                &self.meta_cache,
            )
            .await?;
        if let Some(items) = albums["items"].as_array_mut() {
            dedup_albums(items);
        }
        let ep_singles = self.release_items(artist_id, "EPSANDSINGLES").await;
        let compilations = self.release_items(artist_id, "COMPILATIONS").await;
        if let Some(all) = albums["items"].as_array_mut() {
            if let Some(mut extra) = ep_singles {
                merge_album_sections(all, &mut extra);
            }
            if let Some(mut extra) = compilations {
                merge_compilations(all, &mut extra);
            }
        }
        Ok(albums)
    }

    // An artist's most popular tracks. Backs getTopSongs/top tracks in
    // getArtistInfo and similarSongs. Items come from the v1 toptracks
    // endpoint, whose track objects carry replayGain/peak; the v2
    // relationship never does. Fetching stops once `limit` items are in
    // hand, so the small slices similarSongs asks for cost one page.
    pub async fn artist_top_tracks_parallel(
        client: &'static TidalClient,
        artist_id: u64,
        limit: u32,
    ) -> Result<Value, super::Error> {
        let path = format!("/artists/{artist_id}/toptracks");
        let items = super::v1_prefix(client, &path, &client.meta_cache, &[], 100, limit).await?;
        Ok(serde_json::json!({ "items": items }))
    }

    // An artist's biography. Backs getArtistInfo2. The text carries
    // [wimpLink ...] wiki markup; the handler strips it.
    pub async fn artist_bio(&self, artist_id: u64) -> Result<Value, super::Error> {
        let doc = self
            .openapi_get(
                &format!("/artists/{artist_id}"),
                &[("include", "biography")],
                &self.meta_cache,
            )
            .await?;
        Ok(jsonapi::flatten_resource(&doc["data"], &doc))
    }

    // Artists similar to the given one. Backs getArtistInfo2.
    pub async fn artist_similar(&self, artist_id: u64, _limit: u32) -> Result<Value, super::Error> {
        let mut items: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params: Vec<(&str, &str)> = vec![
                ("include", "similarArtists,similarArtists.profileArt"),
            ];
            if let Some(c) = &cursor {
                params.push(("page[cursor]", c.as_str()));
            }
            let doc = self
                .openapi_get(
                    &format!("/artists/{artist_id}/relationships/similarArtists"),
                    &params,
                    &self.meta_cache,
                )
                .await?;
            items.extend(jsonapi::bare_items(&doc));
            match jsonapi::next_cursor(&doc) {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(serde_json::json!({ "items": items }))
    }

    // Favorited artists, newest first. Backs getStarred/getStarred2.
    pub async fn favorite_artists(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
        self.favorite_pages("Artists", "items,items.profileArt", offset, limit)
            .await
    }

    // --- v1 backups (dead code) ------------------------------------
    #[allow(dead_code)]
    pub async fn artist_v1(&self, id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/artists/{id}"), &self.meta_cache).await
    }

    async fn release_items(&self, artist_id: u64, filter: &str) -> Option<Vec<Value>> {
        let resp = match self
            .get_json_q(
                &format!("/artists/{artist_id}/albums"),
                &[("limit", "1000"), ("offset", "0"), ("filter", filter)],
                &self.meta_cache,
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("tidal artist {filter} fetch failed: {e}");
                return None;
            }
        };
        let mut items = resp["items"].as_array().cloned().unwrap_or_default();
        dedup_albums(&mut items);
        Some(items)
    }

    #[allow(dead_code)]
    pub async fn artist_top_tracks_v1(&self, artist_id: u64, limit: u32) -> Result<Value, super::Error> {
        let limit = limit.to_string();
        self.get_json_q(
            &format!("/artists/{artist_id}/toptracks"),
            &[("limit", limit.as_str()), ("offset", "0")],
            &self.meta_cache,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn artist_bio_v1(&self, artist_id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/artists/{artist_id}/bio"), &self.meta_cache)
            .await
    }

    #[allow(dead_code)]
    pub async fn artist_similar_v1(&self, artist_id: u64, limit: u32) -> Result<Value, super::Error> {
        let limit = limit.to_string();
        self.get_json_q(
            &format!("/artists/{artist_id}/similar"),
            &[("limit", limit.as_str()), ("offset", "0")],
            &self.meta_cache,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn favorite_artists_v1(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
        let user_id = self.user_id().await?;
        let offset = offset.to_string();
        let limit = limit.to_string();
        let path = format!("/users/{user_id}/favorites/artists");
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

// Append EPs and singles to the album list. The EP/singles list is
// deduplicated on its own: a single and an album with the same title
// are different releases and must both stay.
fn merge_album_sections(albums: &mut Vec<Value>, ep_singles: &mut Vec<Value>) {
    dedup_albums(ep_singles);
    albums.append(ep_singles);
}

// Mark every item as a compilation and append the list. Tidal's own
// item type cannot distinguish compilations from albums, so the artist
// client stamps them for the mapper (see album_from_tidal).
fn merge_compilations(albums: &mut Vec<Value>, compilations: &mut Vec<Value>) {
    for item in compilations.iter_mut() {
        item["isCompilation"] = serde_json::json!(true);
    }
    merge_album_sections(albums, compilations);
}

// Within a title group, keep the copy Tidal ranks highest (the
// `popularity` field). That matches the copy Tidal's own search surfaces,
// verified against real artist data (15 of 16 titles). Ties keep the
// first occurrence; the list keeps each title's first-occurrence position.
// Titles compare case-insensitively, with the curly apostrophe normalized:
// Tidal mixes both spellings ("Taylor's" / "Taylor\u{2019}s"). A missing
// title falls back to the album id, so a repeated id still collapses.
fn dedup_albums(items: &mut Vec<Value>) {
    // normalized title -> (index of the best copy, its popularity)
    let mut best: HashMap<String, (usize, u64)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (i, v) in items.iter().enumerate() {
        let key = match v["title"].as_str() {
            Some(title) => title.trim().to_lowercase().replace('\u{2019}', "'"),
            None => v["id"].to_string(),
        };
        let pop = v["popularity"].as_u64().unwrap_or(0);
        match best.entry(key.clone()) {
            Entry::Vacant(e) => {
                order.push(key);
                e.insert((i, pop));
            }
            Entry::Occupied(mut e) if e.get().1 < pop => {
                e.insert((i, pop));
            }
            Entry::Occupied(_) => {}
        }
    }
    *items = order
        .iter()
        .map(|k| items[best[k].0].clone())
        .collect();
}

#[cfg(test)]
mod tests {
    use super::{dedup_albums, merge_album_sections, merge_compilations};
    use serde_json::{json, Value};

    #[test]
    fn dedup_albums_keeps_most_popular_per_title() {
        let mut items: Vec<Value> = vec![
            json!({ "id": 1, "title": "Midnights", "popularity": 26 }),
            json!({ "id": 2, "title": "Midnights", "popularity": 75 }),
            json!({ "id": 3, "title": "Midnights", "popularity": 20 }),
            json!({ "id": 4, "title": "evermore", "popularity": 68 }),
            json!({ "id": 5, "title": "evermore", "popularity": 36 }),
        ];
        dedup_albums(&mut items);
        let ids: Vec<u64> = items.iter().map(|v| v["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, vec![2, 4]);
    }

    #[test]
    fn dedup_albums_keeps_first_on_popularity_tie() {
        let mut items: Vec<Value> = vec![
            json!({ "id": 1, "title": "Lover", "popularity": 54 }),
            json!({ "id": 2, "title": "Lover", "popularity": 54 }),
        ];
        dedup_albums(&mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], 1);
    }

    #[test]
    fn dedup_albums_missing_popularity_keeps_first() {
        let mut items: Vec<Value> = vec![
            json!({ "id": 1, "title": "The Life of a Showgirl" }),
            json!({ "id": 2, "title": "The Life of a Showgirl" }),
            json!({ "id": 3, "title": "The Life of a Showgirl" }),
            json!({ "id": 4, "title": "The Life of a Showgirl (Deluxe)" }),
        ];
        dedup_albums(&mut items);
        let ids: Vec<u64> = items.iter().map(|v| v["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, vec![1, 4]);
    }

    #[test]
    fn dedup_albums_normalizes_curly_apostrophe() {
        let mut items: Vec<Value> = vec![
            json!({ "id": 1, "title": "Red (Taylor's Version)" }),
            json!({ "id": 2, "title": "Red (Taylor\u{2019}s Version)" }),
        ];
        dedup_albums(&mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], 1);
    }

    #[test]
    fn dedup_albums_keeps_distinct_titles() {
        let mut items: Vec<Value> = vec![
            json!({ "id": 1, "title": "Midnights" }),
            json!({ "id": 2, "title": "Midnights (3am Edition)" }),
            json!({ "id": 3, "title": "evermore" }),
        ];
        dedup_albums(&mut items);
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn dedup_albums_falls_back_to_id_when_title_missing() {
        let mut items: Vec<Value> = vec![
            json!({ "id": 1, "title": "A" }),
            json!({ "id": 9 }),
            json!({ "id": 9 }),
            json!({ "id": 10 }),
        ];
        dedup_albums(&mut items);
        assert_eq!(items.len(), 3);
        assert_eq!(items[1]["id"], 9);
    }

    #[test]
    fn merge_keeps_same_title_album_and_single() {
        // A single and an album with the same title are different
        // releases: the merge must keep both, even when the single is
        // more popular and would win a cross-list dedup.
        let mut albums: Vec<Value> = vec![json!({ "id": 1, "title": "Lover", "popularity": 10 })];
        let mut eps: Vec<Value> = vec![json!({ "id": 2, "title": "Lover", "popularity": 90 })];
        merge_album_sections(&mut albums, &mut eps);
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0]["id"], 1);
        assert_eq!(albums[1]["id"], 2);
    }

    #[test]
    fn merge_compilations_stamps_and_appends() {
        let mut albums: Vec<Value> = vec![json!({ "id": 1, "title": "LP" })];
        let mut comps: Vec<Value> = vec![
            json!({ "id": 2, "title": "Best Of" }),
            json!({ "id": 3, "title": "Best Of" }),
        ];
        merge_compilations(&mut albums, &mut comps);
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[1]["isCompilation"], true);
        assert!(albums[0].get("isCompilation").is_none());
    }
}