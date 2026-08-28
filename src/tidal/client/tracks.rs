// Track endpoints. v2 OpenAPI (JSON:API) shapes flatten to the v1 JSON
// the mapping layer reads; the old v1 methods stay as dead-code backups
// under `_v1` names.
use serde_json::Value;

use super::{jsonapi, TidalClient};

// Nested include path for a track plus the album/artist/genre resources
// that enrich it. The spec's own examples use multi-segment paths
// (items.items, similarArtists.albums), so nested includes are accepted.
const TRACK_INCLUDE: &str = "albums.coverArt,artists,genres";

impl TidalClient {
    pub async fn track(&self, id: u64) -> Result<Value, super::Error> {
        let doc = self
            .openapi_get(
                &format!("/tracks/{id}"),
                &[("include", TRACK_INCLUDE)],
                &self.meta_cache,
            )
            .await?;
        Ok(jsonapi::flatten_resource(&doc["data"], &doc))
    }

    // A track's lyrics: plain text plus an LRC subtitle track. Tracks
    // without lyrics return an empty lyrics relationship; the handlers
    // treat the resulting 404 as "no lyrics".
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

    // --- v1 backups (dead code) ------------------------------------
    #[allow(dead_code)]
    pub async fn track_v1(&self, id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/tracks/{id}"), &self.meta_cache).await
    }

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