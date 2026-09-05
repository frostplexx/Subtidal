// Playlist endpoints. v2 OpenAPI (JSON:API) shapes; v1 bodies stay as
// dead-code backups under `_v1` names. The v2 API has no etag
// precondition: mutations address entries by item id, not by position,
// so removals fetch the current items first and delete by itemId.
use serde_json::Value;

use super::{jsonapi, TidalClient};

// Playlist items with everything a track entry needs: the track, its
// artists and cover, and the item id for later removal.
const ITEMS_INCLUDE: &str = "items,items.albums.coverArt,items.artists";
const PLAYLIST_INCLUDE: &str = "coverArt";
// Mix items are addressed through the playlists endpoint: the mix
// collections (my_mixes page, userDailyMixes/me) identify their cards
// with type "playlists", and the collection paths themselves 404 for
// individual mixes. The legacy collection paths stay as fallbacks.
const MIX_COLLECTIONS: [&str; 5] = [
    "playlists",
    "userDailyMixes",
    "userDiscoveryMixes",
    "userNewReleaseMixes",
    "userOfflineMixes",
];

// The (type, id, itemId) triple that addresses one playlist entry for
// removal. The item id is opaque and only the items relationship returns
// it; it cannot change between the GET and the DELETE, so playlists that
// mutate between the two are safe.
pub(crate) type ItemAddr = (String, String, String);

impl TidalClient {
    // The user's playlists, newest first. Backs getPlaylists. The
    // response items are the playlist objects themselves (unlike
    // favorites, no `item` wrapper). This walks the userCollectionPlaylists
    // items relationship, the non-deprecated route for owned playlists.
    // The deprecated /playlists?filter[owners.id]=me route rejects its
    // documented sort values live (observed 400 on sort=-createdAt), so
    // it is not used; its shape and pagination are identical.
    pub async fn user_playlists(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
        let mut items: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params: Vec<(&str, &str)> =
                vec![("sort", "-addedAt"), ("include", "items,items.coverArt")];
            if let Some(c) = &cursor {
                params.push(("page[cursor]", c.as_str()));
            }
            let doc = self
                .openapi_get(
                    "/userCollectionPlaylists/me/relationships/items",
                    &params,
                    &self.playlist_cache,
                )
                .await?;
            for e in jsonapi::flatten_item_entries(&doc, false)["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                if let Some(p) = e.get("item") {
                    items.push(p.clone());
                }
            }
            match jsonapi::next_cursor(&doc) {
                Some(c) => cursor = Some(c),
                None => break,
            }
            // Stop once the requested slice is fully in hand.
            if items.len() >= offset as usize + limit as usize {
                break;
            }
        }
        let start = offset as usize;
        let end = start + limit as usize;
        Ok(serde_json::json!({
            "items": items.into_iter().skip(start).take(end.saturating_sub(start)).collect::<Vec<_>>(),
        }))
    }

    // One mix's tracks. Same { items: [{ item, type }] } wrapper as
    // playlist items; totalNumberOfItems is the full walk count (v2
    // relationship docs carry no total). Mixes regenerate, so their
    // pages live in the short mix_cache, never the 6h meta_cache.
    pub async fn mix_items(
        &self,
        mix_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Value, super::Error> {
        let mut last_error: Option<super::Error> = None;
        for collection in MIX_COLLECTIONS {
            match self
                .mix_items_from(collection, mix_id, offset, limit)
                .await
            {
                Ok(v) => return Ok(v),
                // A missing mix in one collection is not an error: try
                // the next. Any other failure is real.
                Err(super::Error::Tidal(404, _)) | Err(super::Error::Tidal(400, _)) => {
                    last_error = Some(super::Error::Tidal(404, "mix not found".into()));
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_error.unwrap_or_else(|| super::Error::Tidal(404, "mix not found".into())))
    }

    async fn mix_items_from(
        &self,
        collection: &str,
        mix_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Value, super::Error> {
        let mut items: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params: Vec<(&str, &str)> = vec![("include", ITEMS_INCLUDE)];
            if let Some(c) = &cursor {
                params.push(("page[cursor]", c.as_str()));
            }
            let doc = self
                .openapi_get(
                    &format!("/{collection}/{mix_id}/relationships/items"),
                    &params,
                    &self.mix_cache,
                )
                .await?;
            items.extend(
                jsonapi::flatten_item_entries(&doc, false)["items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            match jsonapi::next_cursor(&doc) {
                Some(c) => cursor = Some(c),
                None => break,
            }
            // Stop once the requested slice is fully in hand.
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

    // The user's mixes (Daily Mix, My Mix, Discovery). Backs the mix
    // entries blended into getPlaylists. Mixes regenerate daily, so they
    // live in the short mix_cache, never the 6h meta_cache. This Pages
    // endpoint is private and unchanged.
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
        let doc = self
            .openapi_get(
                &format!("/playlists/{uuid}"),
                &[("include", PLAYLIST_INCLUDE)],
                &self.playlist_cache,
            )
            .await?;
        Ok(jsonapi::flatten_resource(&doc["data"], &doc))
    }

    // Every entry of a playlist: { item, type, meta: { itemId, ... } }.
    // Page-walking with cursor pagination; entries carry their item id so
    // removals address exact entries. Videos and tracks both count here,
    // matching the raw index semantics the v1 handler used.
    pub async fn playlist_all_items(&self, uuid: &str) -> Result<Vec<Value>, super::Error> {
        let mut items: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params: Vec<(&str, &str)> = vec![("include", ITEMS_INCLUDE)];
            if let Some(c) = &cursor {
                params.push(("page[cursor]", c.as_str()));
            }
            let doc = self
                .openapi_get(
                    &format!("/playlists/{uuid}/relationships/items"),
                    &params,
                    &self.playlist_cache,
                )
                .await?;
            items.extend(
                jsonapi::flatten_item_entries(&doc, true)["items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            match jsonapi::next_cursor(&doc) {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(items)
    }

    // --- v1 playlist items, fetched concurrently ---------------------
    // The v2 relationship endpoint pages only by opaque cursor, so its
    // walk is one sequential request per page (a long playlist's ~60-100
    // round trips show up as the 5-6s getPlaylist logs). The v1 items
    // endpoint pages by offset instead, which lets pages run concurrently:
    // offsets are stable for a read-only request. Pages of 100, six in
    // flight, reordered on the way back.
    const V1_ITEMS_LIMIT: u32 = 100;
    const V1_ITEMS_IN_FLIGHT: usize = 6;

    async fn v1_items_page(
        client: &'static TidalClient,
        uuid: &str,
        offset: u32,
    ) -> Result<Value, super::Error> {
        let offset = offset.to_string();
        client
            .get_json_q(
                &format!("/playlists/{uuid}/items"),
                &[("limit", "100"), ("offset", offset.as_str())],
                &client.playlist_cache,
            )
            .await
    }
    // Fetch every item of a playlist through the v1 endpoint, with the
    // pages in flight concurrently. Returns the raw entries: v1 wraps
    // each track as { item, type }, the same shape v2 flatten emits, so
    // callers reuse the same mapper. Without a total on the first page
    // the walk degrades to one page at a time until a short page.
    pub async fn playlist_items_parallel(
        client: &'static TidalClient,
        uuid: &str,
    ) -> Result<Vec<Value>, super::Error> {
        let first = Self::v1_items_page(client, uuid, 0).await?;
        let mut items: Vec<Value> = first["items"].as_array().cloned().unwrap_or_default();
        let Some(total) = first["totalNumberOfItems"].as_u64() else {
            let mut offset = Self::V1_ITEMS_LIMIT;
            loop {
                let page = Self::v1_items_page(client, uuid, offset).await?;
                let batch = page["items"].as_array().cloned().unwrap_or_default();
                let n = batch.len();
                items.extend(batch);
                offset += Self::V1_ITEMS_LIMIT;
                if n < Self::V1_ITEMS_LIMIT as usize || items.len() >= 10_000 {
                    break;
                }
            }
            return Ok(items);
        };
        let mut offset = Self::V1_ITEMS_LIMIT;
        while offset < total as u32 {
            let end = (offset as u64
                + (Self::V1_ITEMS_IN_FLIGHT as u64 * Self::V1_ITEMS_LIMIT as u64))
            .min(total) as u32;
            let mut handles = Vec::with_capacity(Self::V1_ITEMS_IN_FLIGHT);
            for off in (offset..end).step_by(Self::V1_ITEMS_LIMIT as usize) {
                let uuid = uuid.to_string();
                handles.push(tokio::spawn(async move {
                    Self::v1_items_page(client, &uuid, off).await
                }));
            }
            for handle in handles {
                let page = handle.await.map_err(|e| {
                    super::Error::HttpDecode(500, format!("playlist page task failed: {e}"))
                })??;
                items.extend(page["items"].as_array().cloned().unwrap_or_default());
            }
            offset = end;
        }
        Ok(items)
    }

    // After any playlist change, drop the cached playlist objects and
    // item pages so the next read fetches fresh data. The v2 playlist
    // keys start with /playlists (the list query, the detail, the item
    // pages), and the owned-playlist list query lives at
    // /userCollectionPlaylists/me/relationships/items; without clearing
    // both, create/delete results stay hidden for the cache lifetime.
    fn invalidate_playlist_caches(&self) {
        for prefix in ["/playlists", "/userCollectionPlaylists/me"] {
            let _ = self
                .playlist_cache
                .invalidate_entries_if(move |k, _| k.starts_with(prefix));
        }
    }

    // Create a playlist. Backs createPlaylist. The response is the new
    // playlist object, with a uuid and zero tracks.
    pub async fn create_playlist(&self, title: &str, description: Option<&str>) -> Result<Value, super::Error> {
        let mut attributes = serde_json::json!({ "name": title });
        if let Some(d) = description {
            attributes["description"] = serde_json::json!(d);
        }
        let payload = serde_json::json!({
            "data": { "type": "playlists", "attributes": attributes },
        });
        let doc = self
            .openapi_send(reqwest::Method::POST, "/playlists", Some(&payload))
            .await?;
        self.invalidate_playlist_caches();
        Ok(jsonapi::flatten_resource(&doc["data"], &doc))
    }

    // Update playlist metadata (name, description). Backs updatePlaylist
    // and createPlaylist's rename mode. PATCH is partial: only the given
    // fields change. accessType is deliberately omitted: the v1 source
    // has no accessType (only publicPlaylist), so sending it would risk
    // flipping a private playlist public.
    pub async fn update_playlist(
        &self,
        uuid: &str,
        title: Option<&str>,
        description: Option<&str>,
    ) -> Result<(), super::Error> {
        if title.is_none() && description.is_none() {
            return Ok(());
        }
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
        self.openapi_send(reqwest::Method::PATCH, &format!("/playlists/{uuid}"), Some(&body))
            .await?;
        self.invalidate_playlist_caches();
        Ok(())
    }

    // Append tracks to a playlist. Backs updatePlaylist's songIdToAdd and
    // createPlaylist's songId. Requests without an item meta append to
    // the end. Sending several ids in one request is atomic on Tidal's
    // side, so a single POST covers the whole batch.
    pub async fn playlist_add_tracks(&self, uuid: &str, track_ids: &[u64]) -> Result<(), super::Error> {
        if track_ids.is_empty() {
            return Ok(());
        }
        let data: Vec<Value> = track_ids
            .iter()
            .map(|id| {
                serde_json::json!({ "id": id.to_string(), "type": "tracks" })
            })
            .collect();
        let payload = serde_json::json!({ "data": data });
        self.openapi_send(
            reqwest::Method::POST,
            &format!("/playlists/{uuid}/relationships/items"),
            Some(&payload),
        )
        .await?;
        self.invalidate_playlist_caches();
        Ok(())
    }

    // Remove entries by item id. Backs songIndexToRemove and
    // replace_songs. Each address is (type, id, itemId); the caller maps
    // Subsonic track positions to addresses once, through playlist_all_items.
    // Chunked at 100 like the v1 endpoint.
    pub async fn playlist_remove_items(&self, uuid: &str, items: &[ItemAddr]) -> Result<(), super::Error> {
        for chunk in items.chunks(100) {
            let data: Vec<Value> = chunk
                .iter()
                .map(|(rtype, id, item_id)| {
                    serde_json::json!({
                        "id": id,
                        "type": rtype,
                        "meta": { "itemId": item_id },
                    })
                })
                .collect();
            let payload = serde_json::json!({ "data": data });
            self.openapi_send(
                reqwest::Method::DELETE,
                &format!("/playlists/{uuid}/relationships/items"),
                Some(&payload),
            )
            .await?;
            self.invalidate_playlist_caches();
        }
        Ok(())
    }

    // Delete a playlist. Backs deletePlaylist.
    pub async fn delete_playlist(&self, uuid: &str) -> Result<(), super::Error> {
        self.openapi_send(reqwest::Method::DELETE, &format!("/playlists/{uuid}"), None)
            .await?;
        self.invalidate_playlist_caches();
        Ok(())
    }

    // --- v1 backups (dead code) ------------------------------------
    #[allow(dead_code)]
    pub async fn user_playlists_v1(&self, offset: u32, limit: u32) -> Result<Value, super::Error> {
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

    #[allow(dead_code)]
    pub async fn mix_items_v1(
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

    #[allow(dead_code)]
    pub async fn playlist_v1(&self, uuid: &str) -> Result<Value, super::Error> {
        self.get_json(&format!("/playlists/{uuid}"), &self.playlist_cache)
            .await
    }

    #[allow(dead_code)]
    pub async fn playlist_items_v1(&self, uuid: &str, offset: u32, limit: u32) -> Result<Value, super::Error> {
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

    #[allow(dead_code)]
    fn invalidate_playlist_caches_v1(&self, user_id: u64, uuid: &str) {
        let user_prefix = format!("/users/{user_id}/playlists");
        let pl_prefix = format!("/playlists/{uuid}");
        let _ = self.playlist_cache.invalidate_entries_if(move |k, _| {
            k.starts_with(&user_prefix) || k.starts_with(&pl_prefix)
        });
    }

    #[allow(dead_code)]
    pub async fn create_playlist_v1(&self, title: &str, description: Option<&str>) -> Result<Value, super::Error> {
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
        let user_id = body["creator"]["id"].as_u64().unwrap_or(user_id);
        self.invalidate_playlist_caches_v1(user_id, body["uuid"].as_str().unwrap_or(""));
        Ok(body)
    }

    #[allow(dead_code)]
    pub async fn update_playlist_v1(
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
        let current = self.playlist_v1(uuid).await?;
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
            self.invalidate_playlist_caches_v1(user_id, uuid);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }

    #[allow(dead_code)]
    pub async fn playlist_add_tracks_v1(&self, uuid: &str, track_ids: &[u64]) -> Result<(), super::Error> {
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
            self.invalidate_playlist_caches_v1(user_id, uuid);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }

    #[allow(dead_code)]
    pub async fn playlist_remove_indices_v1(
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
            self.invalidate_playlist_caches_v1(user_id, uuid);
        }
        Ok(())
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub async fn delete_playlist_v1(&self, uuid: &str) -> Result<(), super::Error> {
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
            self.invalidate_playlist_caches_v1(user_id, uuid);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }
}