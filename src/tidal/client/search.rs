// Search.
use serde_json::Value;

use super::TidalClient;

impl TidalClient {
    pub async fn search(&self, query: &str) -> Result<Value, super::Error> {
        // limit caps each section's page.
        self.get_json_q(
            "/search",
            &[("query", query), ("limit", "50")],
            &self.search_cache,
        )
        .await
    }
}
