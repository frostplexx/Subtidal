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

    // One mix's tracks. Same { item } wrapper as playlist items, so
    // playlist_song_from_item maps them; totalNumberOfItems is the count.
    pub async fn mix_items(
        &self,
        mix_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Value, super::Error> {
        let offset = offset.to_string();
        let limit = limit.to_string();
        self.get_json_q(
            &format!("/mixes/{mix_id}/items"),
            &[("offset", offset.as_str()), ("limit", limit.as_str())],
            &self.mix_cache,
        )
        .await
    }

    // The user's mixes (Daily Mix, My Mix, Discovery). Backs the mix
    // entries blended into getPlaylists. Mixes regenerate daily, so they
    // live in the short mix_cache, never the 6h meta_cache.
    pub async fn my_mixes(&self) -> Result<Value, super::Error> {
        self.get_json_q(
            "/pages/my_collection_my_mixes",
            &[("deviceType", "BROWSER"), ("locale", "en_US")],
            &self.mix_cache,
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

    // After any playlist change, drop the cached playlist objects and
    // item pages so the next read fetches fresh data.
    fn invalidate_playlist_caches(&self, user_id: u64, uuid: &str) {
        let user_prefix = format!("/users/{user_id}/playlists");
        let pl_prefix = format!("/playlists/{uuid}");
        let _ = self.meta_cache.invalidate_entries_if(move |k, _| {
            k.starts_with(&user_prefix) || k.starts_with(&pl_prefix)
        });
    }

    // Create a playlist. Backs createPlaylist. The response is the new
    // playlist object, with a uuid and zero tracks.
    pub async fn create_playlist(&self, title: &str, description: Option<&str>) -> Result<Value, super::Error> {
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let url = format!("{}/users/{user_id}/playlists", super::API_URL);
        let params: Vec<(&str, &str)> = match description {
            Some(d) => vec![("title", title), ("description", d)],
            None => vec![("title", title)],
        };
        let mut req = self.http.post(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.form(&params).send().await?;
        let status = resp.status();
        let body: Value = resp.json().await?;
        if !status.is_success() {
            return Err(super::Error::Tidal(status.as_u16(), body.to_string()));
        }
        self.invalidate_playlist_caches(user_id, &body["uuid"].as_str().unwrap_or(""));
        Ok(body)
    }

    // Update playlist metadata (title, description). Backs updatePlaylist
    // and createPlaylist's rename mode. No-op when both params are absent.
    pub async fn update_playlist(
        &self,
        uuid: &str,
        title: Option<&str>,
        description: Option<&str>,
    ) -> Result<(), super::Error> {
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let mut params: Vec<(&str, &str)> = Vec::new();
        if let Some(t) = title {
            params.push(("title", t));
        }
        if let Some(d) = description {
            params.push(("description", d));
        }
        if params.is_empty() {
            return Ok(());
        }
        let url = format!("{}/playlists/{uuid}", super::API_URL);
        let mut req = self.http.put(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.form(&params).send().await?;
        let status = resp.status();
        if status.is_success() {
            self.invalidate_playlist_caches(user_id, uuid);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }

    // Append tracks to a playlist. Backs updatePlaylist's songIdToAdd and
    // createPlaylist's songId. itemIds travel comma-joined, per the Tidal
    // v1 contract. The onDupes param is left to the server default.
    pub async fn playlist_add_tracks(&self, uuid: &str, track_ids: &[u64]) -> Result<(), super::Error> {
        if track_ids.is_empty() {
            return Ok(());
        }
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let item_ids: Vec<String> = track_ids.iter().map(|id| format!("track:{id}")).collect();
        let joined = item_ids.join(",");
        let url = format!("{}/playlists/{uuid}/items", super::API_URL);
        let mut req = self.http.put(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.form(&[("itemIds", joined.as_str())]).send().await?;
        let status = resp.status();
        if status.is_success() {
            self.invalidate_playlist_caches(user_id, uuid);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }

    // Remove one item from a playlist. itemId is the Tidal "track:<id>"
    // form returned by playlist_item_id_at. Backs songIndexToRemove.
    pub async fn playlist_remove_item(&self, uuid: &str, item_id: &str) -> Result<(), super::Error> {
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let url = format!("{}/playlists/{uuid}/items/{item_id}", super::API_URL);
        let mut req = self.http.delete(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            self.invalidate_playlist_caches(user_id, uuid);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }

    // Delete a playlist. Backs deletePlaylist.
    pub async fn delete_playlist(&self, uuid: &str) -> Result<(), super::Error> {
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let url = format!("{}/playlists/{uuid}", super::API_URL);
        let mut req = self.http.delete(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            self.invalidate_playlist_caches(user_id, uuid);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }
}
