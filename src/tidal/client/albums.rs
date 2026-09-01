// Album endpoints. v2 OpenAPI (JSON:API) shapes; v1 bodies stay as
// dead-code backups under `_v1` names.
use serde_json::Value;

use super::{jsonapi, TidalClient};

const ALBUM_INCLUDE: &str = "artists,coverArt,genres";

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

    // One album's tracks in track order. Backs getAlbum. Items come
    // from the v1 offset-paged endpoint, whose track objects carry
    // replayGain/peak; the v2 relationships items never do. The v1
    // entries wrap each track in {item, type}.
    pub async fn album_items_parallel(
        client: &'static TidalClient,
        album_id: u64,
    ) -> Result<Value, super::Error> {
        let path = format!("/albums/{album_id}/items");
        let entries =
            super::v1_pages_parallel(client, &path, &client.meta_cache, &[], 100, 6).await?;
        let items: Vec<Value> = entries
            .into_iter()
            .filter_map(|e| e.get("item").cloned())
            .collect();
        Ok(serde_json::json!({ "items": items }))
    }

    // One album plus its tracks in track order, from a single v2
    // document with the items relationship inlined. Loses replayGain/
    // peak (v2 tracks never carry them); used only when the v1
    // detail+items path fails, typically for regionally-unavailable
    // albums the v1 endpoints 404 but the v2 document still resolves.
    pub async fn album_with_items(&self, id: u64) -> Result<Value, super::Error> {
        let doc = self
            .openapi_get(
                &format!("/albums/{id}"),
                &[(
                    "include",
                    "artists,coverArt,genres,items,items.albums.coverArt,items.artists,items.genres",
                )],
                &self.meta_cache,
            )
            .await?;
        let album = jsonapi::flatten_resource(&doc["data"], &doc);
        // JSON:API never repeats the primary resource inside `included`,
        // so item tracks flatten without an album join; the parent album
        // is theirs by construction, so inject it.
        let items = attach_album(jsonapi::relationship_items(&doc["data"], &doc), &album);
        Ok(serde_json::json!({ "album": album, "items": items }))
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

    // --- v1 backups ------------------------------------------------
    pub async fn album_v1(&self, id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/albums/{id}"), &self.meta_cache).await
    }

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

// Stamp the flattened parent album onto tracks that flattened without
// one (see album_with_items). Tracks that already joined an album stay.
fn attach_album(items: Vec<Value>, album: &Value) -> Vec<Value> {
    items
        .into_iter()
        .map(|mut item| {
            if !item["album"].is_object() {
                item["album"] = album.clone();
            }
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // The v2 album document never repeats the parent album in `included`,
    // so flattened item tracks arrive without an album join. attach_album
    // stamps the parent album onto them so song_from_track can map.
    #[test]
    fn attach_album_stamps_tracks_without_one() {
        let album = json!({ "id": 5, "title": "Opus", "cover": "abc-123" });
        let mut items = vec![
            json!({ "id": 7, "title": "Run" }),
            json!({ "id": 8, "title": "Per Aspera", "album": { "id": 9 } }),
        ];
        items = attach_album(items, &album);
        assert_eq!(items[0]["album"]["id"], json!(5));
        assert_eq!(items[0]["album"]["cover"], "abc-123");
        // A track that already joined an album keeps its own.
        assert_eq!(items[1]["album"]["id"], json!(9));
    }
}