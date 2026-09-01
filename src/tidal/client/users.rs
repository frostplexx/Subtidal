// User profile endpoint. v2 has no avatar on users (the Users resource
// carries username, names, country, verification, and so on), so
// get_avatar falls back to its placeholder.
use serde_json::Value;

use super::{jsonapi, TidalClient};

impl TidalClient {
    // Tidal user profile: { username, profileName, firstName, ... }
    pub async fn user_profile(&self) -> Result<Value, super::Error> {
        let doc = self
            .openapi_get("/users/me", &[], &self.meta_cache)
            .await?;
        Ok(jsonapi::flatten_resource(&doc["data"], &doc))
    }

    // --- v1 backup (dead code) -------------------------------------
    #[allow(dead_code)]
    pub async fn user_profile_v1(&self) -> Result<Value, super::Error> {
        let user_id = self.user_id().await?;
        self.get_json(&format!("/users/{user_id}"), &self.meta_cache)
            .await
    }
}