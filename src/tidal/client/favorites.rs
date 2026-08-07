// Favorite mutations (star/unstar): add via POST form, remove via DELETE.
// Both endpoints are idempotent — Tidal answers 200 for a duplicate add
// and for a remove of a non-favorite — so repeat clicks are harmless.
use super::TidalClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FavoriteKind {
    Track,
    Album,
    Artist,
}

impl FavoriteKind {
    // The favorites list path segment.
    fn list(self) -> &'static str {
        match self {
            FavoriteKind::Track => "tracks",
            FavoriteKind::Album => "albums",
            FavoriteKind::Artist => "artists",
        }
    }

    // The form field the POST add endpoint expects.
    fn field(self) -> &'static str {
        match self {
            FavoriteKind::Track => "trackId",
            FavoriteKind::Album => "albumId",
            FavoriteKind::Artist => "artistId",
        }
    }
}

impl TidalClient {
    // After any favorite change, drop the cached favorites lists so the
    // next getStarred/getAlbumList2 read fresh data. The cache key is the
    // full path plus query, so a prefix match covers all three lists.
    fn invalidate_favorites_cache(&self, user_id: u64) {
        let prefix = format!("/users/{user_id}/favorites");
        let _ = self
            .meta_cache
            .invalidate_entries_if(move |k, _| k.starts_with(&prefix));
    }

    pub async fn add_favorite(&self, kind: FavoriteKind, id: u64) -> Result<(), super::Error> {
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let id_str = id.to_string();
        let url = format!("{}/users/{user_id}/favorites/{}", super::API_URL, kind.list());
        let mut req = self.http.post(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.form(&[(kind.field(), id_str.as_str())]).send().await?;
        let status = resp.status();
        if status.is_success() {
            self.invalidate_favorites_cache(user_id);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }

    pub async fn remove_favorite(&self, kind: FavoriteKind, id: u64) -> Result<(), super::Error> {
        let user_id = self.user_id().await?;
        let token = self.access_token().await?;
        let url = format!(
            "{}/users/{user_id}/favorites/{}/{}",
            super::API_URL,
            kind.list(),
            id
        );
        let mut req = self.http.delete(&url).bearer_auth(token);
        if let Some(cc) = self.country_code().await? {
            req = req.query(&[("countryCode", cc)]);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            self.invalidate_favorites_cache(user_id);
            Ok(())
        } else {
            Err(super::Error::Tidal(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ))
        }
    }
}
