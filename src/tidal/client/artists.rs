// Artist endpoints.
use serde_json::Value;

use super::TidalClient;

impl TidalClient {
    pub async fn artist(&self, id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/artists/{id}"), &self.meta_cache).await
    }

    // One artist's albums. Backs getArtist. The page cap is far above any
    // artist's catalog, so one call returns everything.
    pub async fn artist_albums(&self, artist_id: u64) -> Result<Value, super::Error> {
        self.get_json_q(
            &format!("/artists/{artist_id}/albums"),
            &[("limit", "1000"), ("offset", "0")],
            &self.meta_cache,
        )
        .await
    }

    // An artist's most popular tracks. Backs getTopSongs.
    pub async fn artist_top_tracks(&self, artist_id: u64, limit: u32) -> Result<Value, super::Error> {
        let limit = limit.to_string();
        self.get_json_q(
            &format!("/artists/{artist_id}/toptracks"),
            &[("limit", limit.as_str()), ("offset", "0")],
            &self.meta_cache,
        )
        .await
    }

    // An artist's biography. Backs getArtistInfo2. The text carries
    // [wimpLink ...] wiki markup; the handler strips it.
    pub async fn artist_bio(&self, artist_id: u64) -> Result<Value, super::Error> {
        self.get_json(&format!("/artists/{artist_id}/bio"), &self.meta_cache)
            .await
    }

    // Artists similar to the given one. Backs getArtistInfo2.
    pub async fn artist_similar(&self, artist_id: u64, limit: u32) -> Result<Value, super::Error> {
        let limit = limit.to_string();
        self.get_json_q(
            &format!("/artists/{artist_id}/similar"),
            &[("limit", limit.as_str()), ("offset", "0")],
            &self.meta_cache,
        )
        .await
    }

    // Favorited artists, newest first. Backs getStarred/getStarred2.
    // Same { item, created } wrapper shape as favorite_albums.
    pub async fn favorite_artists(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
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
