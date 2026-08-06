// Track endpoints.
use serde_json::Value;

use super::TidalClient;

impl TidalClient {
    pub async fn track(&self, id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/tracks/{id}"), &self.meta_cache).await
    }

    // Favorited tracks, newest first. Backs getStarred/getStarred2.
    // Same { item, created } wrapper shape as favorite_albums.
    pub async fn favorite_tracks(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
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
