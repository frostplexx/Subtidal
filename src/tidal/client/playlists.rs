// Playlist endpoints.
use serde_json::Value;

use super::TidalClient;

impl TidalClient {
    // The user's playlists, newest first. Backs getPlaylists. The response
    // items are the playlist objects themselves (unlike favorites, no
    // `item` wrapper).
    pub async fn user_playlists(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
        let user_id = self.user_id().await?;
        let offset = offset.to_string();
        let limit = limit.to_string();
        let path = format!("/users/{user_id}/playlists");
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

    // One playlist by UUID. Backs getCoverArt for playlist covers.
    pub async fn playlist(&self, uuid: &str) -> Result<Value, super::Error> {
        self.get_json(&format!("/playlists/{uuid}"), &self.meta_cache)
            .await
    }

    // One page of a playlist's tracks, wrapped as { item: { ...track }, type }.
    pub async fn playlist_items(&self, uuid: &str, offset: u32, limit: u32) -> Result<Value, super::Error> {
        let offset = offset.to_string();
        let limit = limit.to_string();
        let path = format!("/playlists/{uuid}/items");
        self.get_json_q(
            &path,
            &[("limit", limit.as_str()), ("offset", offset.as_str())],
            &self.meta_cache,
        )
        .await
    }
}
