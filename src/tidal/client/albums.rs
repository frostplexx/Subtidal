// Album endpoints. v2 OpenAPI (JSON:API) shapes; v1 bodies stay as
// dead-code backups under `_v1` names.
use serde_json::Value;

use super::{jsonapi, TidalClient};

const ALBUM_INCLUDE: &str = "artists,coverArt,genres";
// Album items embed the tracks plus their artists/cover sources; the
// identifier meta supplies track/volume numbers.
const ALBUM_ITEMS_INCLUDE: &str = "items,items.albums.coverArt,items.artists,items.genres";

impl TidalClient {
    pub async fn album(&self, id: u64) -> Result<Value, super::Error> {
        let doc = self
            .openapi_get(
                &format!("/albums/{id}"),
                &[("include", ALBUM_INCLUDE)],
                &self.meta_cache,
            )
            .await?;
        Ok(jsonapi::flatten_resource(&doc["data"], &doc))
    }

    // One album's tracks in track order. Backs getAlbum. Response items
    // are the bare flattened track objects (the shape v1 returned), with
    // trackNumber/volumeNumber injected from the item identifiers.
    pub async fn album_tracks(&self, album_id: u64) -> Result<Value, super::Error> {
        let path = format!("/albums/{album_id}/relationships/items");
        self.walk_bare_items(&path).await
    }

    // Favorited albums, newest first. Backs getAlbumList2 (type=starred).
    pub async fn favorite_albums(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
        self.favorite_pages(
            "Albums",
            "items,items.artists,items.coverArt,items.genres",
            offset,
            limit,
        )
        .await
    }

    // Walk a relationship-items endpoint page by page and return the
    // flattened resources directly, one per item. Shared by album tracks
    // and artist top tracks, whose v1 shapes were bare item lists.
    async fn walk_bare_items(&self, path: &str) -> Result<Value, super::Error> {
        let mut items: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params: Vec<(&str, &str)> = vec![("include", ALBUM_ITEMS_INCLUDE)];
            if let Some(c) = &cursor {
                params.push(("page[cursor]", c.as_str()));
            }
            let doc = self.openapi_get(path, &params, &self.meta_cache).await?;
            items.extend(jsonapi::bare_items(&doc));
            match jsonapi::next_cursor(&doc) {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(serde_json::json!({ "items": items }))
    }

    // --- v1 backups (dead code) ------------------------------------
    #[allow(dead_code)]
    pub async fn album_v1(&self, id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/albums/{id}"), &self.meta_cache).await
    }

    #[allow(dead_code)]
    pub async fn album_tracks_v1(&self, album_id: u64) -> Result<Value, super::Error> {
        self.get_json_q(
            &format!("/albums/{album_id}/tracks"),
            &[("limit", "1000"), ("offset", "0")],
            &self.meta_cache,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn favorite_albums_v1(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
        let user_id = self.user_id().await?;
        let offset = offset.to_string();
        let limit = limit.to_string();
        let path = format!("/users/{user_id}/favorites/albums");
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