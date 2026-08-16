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
            &self.playlist_cache,
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
        self.get_json(&format!("/playlists/{uuid}"), &self.playlist_cache)
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
            &self.playlist_cache,
        )
        .await
    }

    // After any playlist change, drop the cached playlist objects and
    // item pages so the next read fetches fresh data.
    fn invalidate_playlist_caches(&self, user_id: u64, uuid: &str) {
        let user_prefix = format!("/users/{user_id}/playlists");
        let pl_prefix = format!("/playlists/{uuid}");
        let _ = self.playlist_cache.invalidate_entries_if(move |k, _| {
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
        self.invalidate_playlist_caches(user_id, body["uuid"].as_str().unwrap_or(""));
        Ok(body)
    }

    // Update playlist metadata (name, description). Backs updatePlaylist
    // and createPlaylist's rename mode. Uses the OpenAPI JSON:API PATCH on
    // openapi.tidal.com, as Sone does; the v1 API registers no PUT here
    // (it answers 405). When only one field is given, the other comes from
    // the current playlist: the body always carries both name and
    // description. accessType is deliberately omitted: PATCH updates are
    // partial, and the v1 source has no accessType (only publicPlaylist),
    // so sending it would risk flipping a private playlist public.
    pub async fn update_playlist(
        &self,
        uuid: &str,
        title: Option<&str>,
        description: Option<&str>,
    ) -> Result<(), super::Error> {
        if title.is_none() && description.is_none() {
            return Ok(());
        }
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let current = self.playlist(uuid).await?;
        let name = title.unwrap_or(current["title"].as_str().unwrap_or(""));
        let description = description.unwrap_or(current["description"].as_str().unwrap_or(""));
        let body = serde_json::json!({
            "data": {
                "id": uuid,
                "type": "playlists",
                "attributes": {
                    "name": name,
                    "description": description,
                },
            },
        });
        let url = format!("{}/playlists/{uuid}", super::OPENAPI_URL);
        let mut req = self
            .http
            .patch(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/vnd.api+json");
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.json(&body).send().await?;
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
    // createPlaylist's songId. The v1 items endpoint wants bare track ids
    // (trackIds, no "track:" prefix) via POST, plus an If-None-Match
    // precondition from a fresh GET. onDupes=SKIP and
    // onArtifactNotFound=SKIP keep one duplicate or missing id from
    // failing the whole batch.
    pub async fn playlist_add_tracks(&self, uuid: &str, track_ids: &[u64]) -> Result<(), super::Error> {
        if track_ids.is_empty() {
            return Ok(());
        }
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let etag = self.playlist_etag(uuid).await?;
        let track_ids: Vec<String> = track_ids.iter().map(|id| id.to_string()).collect();
        let joined = track_ids.join(",");
        let url = format!("{}/playlists/{uuid}/items", super::API_URL);
        let mut req = self
            .http
            .post(&url)
            .bearer_auth(token)
            .header("If-None-Match", etag);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req
            .form(&[
                ("trackIds", joined.as_str()),
                ("onDupes", "SKIP"),
                ("onArtifactNotFound", "SKIP"),
            ])
            .send()
            .await?;
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

    // Remove items at raw positions (0-based indices in the items array,
    // tracks and videos alike). Backs songIndexToRemove and replace_songs.
    // The v1 endpoint deletes by index; comma-joined indices travel in one
    // request (chunked at 100 to keep URLs short). Every chunk needs a
    // fresh If-None-Match precondition, so the etag is fetched per chunk.
    pub async fn playlist_remove_indices(
        &self,
        uuid: &str,
        indices: &[u32],
    ) -> Result<(), super::Error> {
        if indices.is_empty() {
            return Ok(());
        }
        let user_id = self.user_id().await?;
        let mut sorted: Vec<u32> = indices.to_vec();
        sorted.sort_unstable();
        for chunk in sorted.chunks(100) {
            let token = self.access_token().await?;
            let etag = self.playlist_etag(uuid).await?;
            let joined: Vec<String> = chunk.iter().map(|i| i.to_string()).collect();
            let url = format!(
                "{}/playlists/{uuid}/items/{}",
                super::API_URL,
                joined.join(",")
            );
            let mut req = self
                .http
                .delete(&url)
                .bearer_auth(token)
                .header("If-None-Match", etag);
            if let Some(cc) = self.country_code().await? {
                req = req.query(&[("countryCode", cc)]);
            }
            let resp = req.send().await?;
            let status = resp.status();
            if !status.is_success() {
                return Err(super::Error::Tidal(
                    status.as_u16(),
                    resp.text().await.unwrap_or_default(),
                ));
            }
            self.invalidate_playlist_caches(user_id, uuid);
        }
        Ok(())
    }

    // A fresh etag for a playlist, from the playlist GET. Tidal's item
    // mutations require the If-None-Match precondition; "*" is the
    // fallback when the header is absent.
    async fn playlist_etag(&self, uuid: &str) -> Result<String, super::Error> {
        let token = self.access_token().await?;
        let url = format!("{}/playlists/{uuid}", super::API_URL);
        let mut req = self.http.get(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ));
        }
        Ok(resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .unwrap_or_else(|| "*".to_string()))
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
