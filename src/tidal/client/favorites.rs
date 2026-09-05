// Favorite mutations (star/unstar): add via POST, remove via DELETE on
// the v2 userCollection relationship endpoints. Both endpoints are
// idempotent — Tidal answers 200/204 for a duplicate add and for a
// remove of a non-favorite — so repeat clicks are harmless.
use super::TidalClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FavoriteKind {
    Track,
    Album,
    Artist,
}

impl FavoriteKind {
    // The userCollection resource path segment.
    fn collection(self) -> &'static str {
        match self {
            FavoriteKind::Track => "Tracks",
            FavoriteKind::Album => "Albums",
            FavoriteKind::Artist => "Artists",
        }
    }

    // The JSON:API resource type of the favorited item.
    fn resource_type(self) -> &'static str {
        match self {
            FavoriteKind::Track => "tracks",
            FavoriteKind::Album => "albums",
            FavoriteKind::Artist => "artists",
        }
    }
}

impl TidalClient {
    // After any favorite change, drop the cached favorites lists so the
    // next getStarred/getAlbumList2 read fresh data. The cache key is the
    // full path plus query, so a prefix match covers all pages of a list.
    // Album and artist reads walk the v2 userCollection endpoints; the
    // track reads use the v1 /users/{id}/favorites/tracks endpoint (v1
    // track objects carry replayGain), so three prefixes must be cleared.
    fn invalidate_favorites_cache(&self) {
        for prefix in [
            "/userCollectionTracks/me",
            "/userCollectionAlbums/me",
            "/userCollectionArtists/me",
        ] {
            let _ = self
                .meta_cache
                .invalidate_entries_if(move |k, _| k.starts_with(prefix));
        }
        // The v1 favorite-tracks key embeds the user id. The mutation just
        // ran with a valid token, so the stored id is available.
        if let Some(uid) = self.user_id_from_tokens() {
            let v1_tracks = format!("/users/{uid}/favorites/tracks");
            let _ = self
                .meta_cache
                .invalidate_entries_if(move |k, _| k.starts_with(&v1_tracks));
        }
    }

    pub async fn add_favorite(&self, kind: FavoriteKind, id: u64) -> Result<(), super::Error> {
        self.toggle_favorite(kind, id, reqwest::Method::POST).await
    }

    pub async fn remove_favorite(&self, kind: FavoriteKind, id: u64) -> Result<(), super::Error> {
        self.toggle_favorite(kind, id, reqwest::Method::DELETE).await
    }

    async fn toggle_favorite(
        &self,
        kind: FavoriteKind,
        id: u64,
        method: reqwest::Method,
    ) -> Result<(), super::Error> {
        let payload = serde_json::json!({
            "data": [{
                "id": id.to_string(),
                "type": kind.resource_type(),
            }],
        });
        let path = format!(
            "/userCollection{}/me/relationships/items",
            kind.collection()
        );
        self.openapi_send(method, &path, Some(&payload)).await?;
        self.invalidate_favorites_cache();
        Ok(())
    }

    // --- v1 backups (dead code) ------------------------------------
    #[allow(dead_code)]
    fn invalidate_favorites_cache_v1(&self, user_id: u64) {
        let prefix = format!("/users/{user_id}/favorites");
        let _ = self
            .meta_cache
            .invalidate_entries_if(move |k, _| k.starts_with(&prefix));
    }

    #[allow(dead_code)]
    pub async fn add_favorite_v1(&self, kind: FavoriteKind, id: u64) -> Result<(), super::Error> {
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let id_str = id.to_string();
        let field = match kind {
            FavoriteKind::Track => "trackId",
            FavoriteKind::Album => "albumId",
            FavoriteKind::Artist => "artistId",
        };
        let list = kind.collection().to_lowercase();
        let url = format!("{}/users/{user_id}/favorites/{list}", super::API_URL);
        let mut req = self.http.post(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.form(&[(field, id_str.as_str())]).send().await?;
        let status = resp.status();
        if status.is_success() {
            self.invalidate_favorites_cache_v1(user_id);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }

    #[allow(dead_code)]
    pub async fn remove_favorite_v1(&self, kind: FavoriteKind, id: u64) -> Result<(), super::Error> {
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let list = kind.collection().to_lowercase();
        let url = format!(
            "{}/users/{user_id}/favorites/{list}/{id}",
            super::API_URL
        );
        let mut req = self.http.delete(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            self.invalidate_favorites_cache_v1(user_id);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }
}