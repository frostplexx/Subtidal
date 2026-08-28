// Genre catalog. Tidal v2 exposes genres as a resource list carrying
// only a name. The Subsonic genre response requires counts (clients
// validate them as numbers), so each genre's counts come from the v1
// per-genre album/track endpoints. The v1 endpoints are keyed by the
// genre's browse path ("Hiphop", "Funk") rather than the numeric id,
// so the v1 /genres list supplies a name->path map; simple names fall
// back to using the name itself. Count fetches run concurrently and
// are cached; a count failure degrades to zero without failing the
// row.
use std::collections::HashMap;

use serde_json::{json, Value};

use super::TidalClient;

// Genre rows: [{id, name, albumCount, songCount}]. Backs getGenres.
pub async fn genre_list(client: &'static TidalClient) -> Result<Vec<Value>, super::Error> {
    let (doc, paths_doc) = tokio::join!(
        client.openapi_get(
            "/genres",
            &[("filter[id]", "USER_SELECTABLE")],
            &client.meta_cache,
        ),
        client.get_json("/genres", &client.meta_cache),
    );
    let doc = doc?;
    let paths = genre_paths(&paths_doc?);
    let rows = genre_rows(&doc);
    let mut out = Vec::with_capacity(rows.len());
    // Six count requests in flight at once; reordered on the way back.
    let mut i = 0;
    while i < rows.len() {
        let end = (i + 6).min(rows.len());
        let mut handles = Vec::with_capacity(end - i);
        for r in &rows[i..end] {
            let name = r["name"].as_str().unwrap_or("").to_string();
            let key = count_key(&name, &paths);
            handles.push(tokio::spawn(async move { genre_counts(client, &key).await }));
        }
        for (r, h) in rows[i..end].iter().zip(handles) {
            let (album_count, song_count) = h.await.map_err(|e| {
                super::Error::HttpDecode(500, format!("genre counts task failed: {e}"))
            })??;
            out.push(json!({
                "id": r["id"],
                "name": r["name"],
                "albumCount": album_count,
                "songCount": song_count,
            }));
        }
        i = end;
    }
    Ok(out)
}

// Map a v1 /genres document to a name->path table. Returns an empty
// map when the document is malformed, so counts fall back to names.
fn genre_paths(doc: &Value) -> HashMap<String, String> {
    doc.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|g| {
                    let name = normalize(g["name"].as_str()?);
                    let path = g["path"].as_str()?;
                    Some((name, path.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

// The count endpoint key for a genre name: the v1 browse path when one
// exists ("Hip Hop/Rap" -> "Hiphop"), otherwise the name itself.
fn count_key(genre_name: &str, paths: &HashMap<String, String>) -> String {
    paths
        .get(&normalize(genre_name))
        .cloned()
        .unwrap_or_else(|| genre_name.to_string())
}

// The v1 genre list spaces the slashes that v2 leaves out; fold both
// spellings onto one key ("Hip Hop / Rap" == "Hip Hop/Rap").
fn normalize(name: &str) -> String {
    name.trim().replace(" /", "/").replace("/ ", "/").replace("  ", " ")
}

// Counts for one genre: the v1 /genres/{key}/albums and /tracks pages
// carry totalNumberOfItems. Either count failure degrades to zero.
async fn genre_counts(client: &'static TidalClient, key: &str) -> Result<(u32, u32), super::Error> {
    let albums = client
        .get_json_q(
            &format!("/genres/{key}/albums"),
            &[("limit", "1")],
            &client.meta_cache,
        )
        .await;
    let tracks = client
        .get_json_q(
            &format!("/genres/{key}/tracks"),
            &[("limit", "1")],
            &client.meta_cache,
        )
        .await;
    let album_count = match albums {
        Ok(v) => v["totalNumberOfItems"].as_u64().unwrap_or(0) as u32,
        Err(e) => {
            tracing::debug!("genre album count fetch failed for {key}: {e}");
            0
        }
    };
    let song_count = match tracks {
        Ok(v) => v["totalNumberOfItems"].as_u64().unwrap_or(0) as u32,
        Err(e) => {
            tracing::debug!("genre track count fetch failed for {key}: {e}");
            0
        }
    };
    Ok((album_count, song_count))
}

// Map a genres document to rows of id + name.
fn genre_rows(doc: &Value) -> Vec<Value> {
    doc["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|g| {
                    let id = g["id"].as_str()?;
                    let name = g["attributes"]["genreName"].as_str()?;
                    Some(json!({ "id": id, "name": name }))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_selectable_genres_map_to_rows() {
        let doc = serde_json::json!({
            "data": [
                {"type": "genres", "id": "1", "attributes": {"genreName": "pop"}},
                {"type": "genres", "id": "2", "attributes": {"genreName": "hip hop"}}
            ],
            "links": {"self": "https://openapi.tidal.com/genres?filter%5Bid%5D=USER_SELECTABLE"}
        });
        assert_eq!(
            genre_rows(&doc),
            vec![
                json!({"id": "1", "name": "pop"}),
                json!({"id": "2", "name": "hip hop"})
            ]
        );
    }

    #[test]
    fn empty_data_yields_empty_list() {
        let doc = serde_json::json!({ "data": [], "links": {} });
        assert!(genre_rows(&doc).is_empty());
    }

    #[test]
    fn v1_path_doc_maps_to_name_path_table() {
        let doc = serde_json::json!([
            {"name": "Pop", "path": "Pop"},
            {"name": "Hip Hop / Rap", "path": "Hiphop"},
            {"name": "R&B / Soul", "path": "Funk"},
            {"name": "World Music", "path": "World"}
        ]);
        let paths = genre_paths(&doc);
        assert_eq!(paths.get("Pop").map(String::as_str), Some("Pop"));
        assert_eq!(
            paths.get("Hip Hop/Rap").map(String::as_str),
            Some("Hiphop")
        );
        assert_eq!(paths.get("R&B/Soul").map(String::as_str), Some("Funk"));
        assert_eq!(paths.get("World Music").map(String::as_str), Some("World"));
        // A malformed document degrades to an empty table, never an error.
        assert!(genre_paths(&json!({})).is_empty());
    }

    #[test]
    fn count_key_uses_path_then_name_fallback() {
        let paths = genre_paths(&json!([
            {"name": "Hip Hop / Rap", "path": "Hiphop"},
            {"name": "Pop", "path": "Pop"}
        ]));
        assert_eq!(count_key("Hip Hop/Rap", &paths), "Hiphop");
        assert_eq!(count_key("Pop", &paths), "Pop");
        // Genres only in v2 keep their own name as the key; the endpoint
        // 404s and the count degrades to zero.
        assert_eq!(count_key("Alternative", &paths), "Alternative");
        assert_eq!(count_key("Alternative", &HashMap::new()), "Alternative");
    }

    #[test]
    fn normalize_folds_slash_spacing() {
        assert_eq!(normalize(" Hip Hop / Rap "), "Hip Hop/Rap");
        assert_eq!(normalize("R&B / Soul"), "R&B/Soul");
        assert_eq!(normalize("Pop"), "Pop");
    }
}