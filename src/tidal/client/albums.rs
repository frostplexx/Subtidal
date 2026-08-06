// Album endpoints.
use serde_json::Value;

use super::TidalClient;

impl TidalClient {
    pub async fn album(&self, id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/albums/{id}"), &self.meta_cache).await
    }

    // One album's tracks in track order. Backs getAlbum. The page cap is
    // far above any album's track count, so one call returns everything.
    pub async fn album_tracks(&self, album_id: u64) -> Result<Value, super::Error> {
        self.get_json_q(
            &format!("/albums/{album_id}/tracks"),
            &[("limit", "1000"), ("offset", "0")],
            &self.meta_cache,
        )
        .await
    }

    // Favorited albums, newest first. Backs getAlbumList2 (type=starred).
    // Response: { items: [{ item: {album}, created }], totalNumberOfItems, ... }
    pub async fn favorite_albums(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
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
