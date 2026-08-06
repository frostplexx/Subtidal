// User profile endpoint.
use serde_json::Value;

use super::TidalClient;

impl TidalClient {
    // Tidal user profile: { username, profileName, firstName, ... }
    pub async fn user_profile(&self) -> Result<Value, super::Error> {
        let user_id = self.user_id().await?;
        self.get_json(&format!("/users/{user_id}"), &self.meta_cache)
            .await
    }
}
