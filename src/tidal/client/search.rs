// Search. v2 /searchResults with nested includes so the single document
// carries everything; v1 body stays as dead-code backup.
use serde_json::Value;

use super::{jsonapi, TidalClient};

// Nested includes pull each section's resources plus their enrichment.
// The searchResults resource's own relationships carry the per-category
// identifiers in order; the flatten read resolves by relationship, never
// by scanning `included` by type (that would mix categories). The API
// caps the include list at 10 entries (a 10-token list fails with
// "Include count 11 exceeds limit 10"), so playlist covers are traded
// away to stay at 9; playlists themselves keep returning.
const SEARCH_INCLUDE: &str =
    "albums,albums.artists,albums.coverArt,artists,artists.profileArt,playlists,tracks,tracks.albums.coverArt,tracks.artists";

impl TidalClient {
    pub async fn search(&self, query: &str) -> Result<Value, super::Error> {
        let doc = self
            .openapi_get(
                "/searchResults",
                &[
                    ("filter[query]", query),
                    ("include", SEARCH_INCLUDE),
                    ("explicitFilter", "INCLUDE"),
                ],
                &self.search_cache,
            )
            .await?;
        Ok(jsonapi::flatten_search(&doc))
    }

    // --- v1 backup (dead code) -------------------------------------
    #[allow(dead_code)]
    pub async fn search_v1(&self, query: &str) -> Result<Value, super::Error> {
        self.get_json_q(
            "/search",
            &[("query", query), ("limit", "50")],
            &self.search_cache,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The search API rejects include lists of 10 or more entries
    // (observed: 10 tokens fail with "Include count 11 exceeds limit
    // 10"). Guard the list so search never regresses to a hard 400.
    #[test]
    fn search_include_stays_under_the_api_limit() {
        assert!(SEARCH_INCLUDE.split(',').count() <= 9);
        // The four sections must all still be present.
        for section in ["albums", "artists", "playlists", "tracks"] {
            assert!(SEARCH_INCLUDE.split(',').any(|i| i == section));
        }
    }
}