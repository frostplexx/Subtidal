// Artist and track radio: Tidal's generated "radio" mixes, seeded from
// one artist or one track. The v1 /mix endpoints hand back the mix id;
// its tracks come from the v1 mix items list. Radio regenerates, so
// both live in the short mix_cache.
use serde_json::Value;

use super::TidalClient;

impl TidalClient {
    pub async fn artist_radio(&self, artist_id: u64, limit: u32) -> Result<Vec<Value>, super::Error> {
        self.radio(&format!("/artists/{artist_id}/mix"), limit).await
    }

    pub async fn track_radio(&self, track_id: u64, limit: u32) -> Result<Vec<Value>, super::Error> {
        self.radio(&format!("/tracks/{track_id}/mix"), limit).await
    }

    async fn radio(&self, mix_path: &str, limit: u32) -> Result<Vec<Value>, super::Error> {
        let mix = self.get_json(mix_path, &self.mix_cache).await?;
        let Some(mix_id) = mix["id"].as_str() else {
            return Err(super::Error::Tidal(404, "no radio mix for this seed".into()));
        };
        let page = self.mix_items_v1(mix_id, 0, limit).await?;
        Ok(page["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter(|e| e["type"].as_str().is_none_or(|t| t == "track"))
                    .filter_map(|e| e.get("item").cloned())
                    .collect()
            })
            .unwrap_or_default())
    }
}
