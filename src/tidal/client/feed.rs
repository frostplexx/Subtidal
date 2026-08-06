// Home feed and album extraction from Pages API responses.
use std::collections::HashSet;

use serde_json::Value;

use super::TidalClient;

impl TidalClient {
    // Personalized home feed. Backs getAlbumList2 (type=newest). The feed
    // includes the "Suggested new albums for you" section, so the whole
    // feed is walked and deduplicated.
    pub async fn home_feed(&self, slug: &str) -> Result<Value, super::Error> {
        self.get_json_q_v2(
            &format!("/home/feed/{slug}"),
            &[
                ("deviceType", "BROWSER"),
                ("locale", "en_US"),
                ("platform", "WEB"),
            ],
            &self.meta_cache,
        )
        .await
    }
}

// Extract album objects from a Pages API or home-feed response. Handles
// every documented layout: V1 rows[].modules[].pagedList.items[], V2
// items[].items[] with { type: "ALBUM", data }, and tabs wrapping either.
// Items wrapped as { item, type } unwrap; duplicates by numeric id drop.
pub(crate) fn albums_from_page(page: &Value) -> Vec<Value> {
    let mut candidates: Vec<Value> = Vec::new();
    collect_v1_rows(page.get("rows"), &mut candidates);
    collect_v2_sections(page.get("items"), &mut candidates);
    if let Some(tabs) = page.get("tabs").and_then(|t| t.as_array()) {
        for tab in tabs {
            collect_v1_rows(tab.get("rows"), &mut candidates);
            collect_v2_sections(tab.get("items"), &mut candidates);
        }
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|v| {
            let album = v.get("item").cloned().unwrap_or(v);
            let id = album.get("id").and_then(|i| i.as_u64())?;
            if seen.insert(id) {
                Some(album)
            } else {
                None
            }
        })
        .collect()
}

// V1 rows: rows[].modules[].pagedList.items[].
fn collect_v1_rows(rows: Option<&Value>, out: &mut Vec<Value>) {
    if let Some(rows) = rows.and_then(|r| r.as_array()) {
        for row in rows {
            if let Some(modules) = row.get("modules").and_then(|m| m.as_array()) {
                for module in modules {
                    if let Some(items) = module
                        .get("pagedList")
                        .and_then(|p| p.get("items"))
                        .and_then(|i| i.as_array())
                    {
                        out.extend(items.iter().cloned());
                    }
                }
            }
        }
    }
}

// V2 sections: items[].items[] where the item type is ALBUM.
fn collect_v2_sections(sections: Option<&Value>, out: &mut Vec<Value>) {
    if let Some(sections) = sections.and_then(|s| s.as_array()) {
        for section in sections {
            if let Some(items) = section.get("items").and_then(|i| i.as_array()) {
                for item in items {
                    if item.get("type").and_then(|t| t.as_str()) == Some("ALBUM") {
                        if let Some(data) = item.get("data") {
                            out.push(data.clone());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::albums_from_page;
    use serde_json::json;

    #[test]
    fn albums_from_page_handles_v1_v2_tabs_and_dedup() {
        let page = json!({
            "rows": [{"modules": [{"type": "ALBUM_LIST", "pagedList": {"items": [
                {"id": 1, "title": "One", "artist": {"id": 9, "name": "X"}},
                {"item": {"id": 2, "title": "Two", "artist": {"id": 9, "name": "X"}}, "type": "ALBUM"}
            ]}}]}],
            "items": [{"type": "HORIZONTAL_LIST", "items": [
                {"type": "ALBUM", "data": {"id": 3, "title": "Three", "artist": {"id": 9, "name": "X"}}},
                {"type": "TRACK", "data": {"id": 4}},
                {"type": "ALBUM", "data": {"id": 1, "title": "One duplicate", "artist": {"id": 9, "name": "X"}}}
            ]}],
            "tabs": [{"rows": [{"modules": [{"type": "ALBUM_LIST", "pagedList": {"items": [
                {"id": 5, "title": "Five", "artist": {"id": 9, "name": "X"}}
            ]}}]}], "items": [{"type": "HORIZONTAL_LIST", "items": [
                {"type": "ALBUM", "data": {"id": 6, "title": "Six", "artist": {"id": 9, "name": "X"}}}
            ]}]}]
        });
        let albums = albums_from_page(&page);
        let ids: Vec<u64> = albums.iter().map(|a| a["id"].as_u64().unwrap()).collect();
        // 1 appears twice (V1 and V2); tracks are skipped; tabs are walked.
        assert_eq!(ids, vec![1, 2, 3, 5, 6]);
    }
}
