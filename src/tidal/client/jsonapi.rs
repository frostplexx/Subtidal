// v2 OpenAPI responses use the JSON:API envelope: `{data, included,
// links, meta}`. The mapping layer consumes v1-shaped JSON, so every
// v2 document passes through this module first. It flattens each
// resource: attributes merged with resolved relationship resources,
// numeric ids, ISO-8601 durations to seconds, and v2 attribute names
// to the v1 names the mappers read.
use std::collections::HashMap;

use serde_json::{json, Value};

// A map of `type:id` -> resource object, built from a document's
// `included` array. Relationships only carry identifiers; the linked
// resources live here.
pub(crate) type Index = HashMap<String, Value>;

pub(crate) fn index(doc: &Value) -> Index {
    let mut idx = Index::new();
    if let Some(included) = doc["included"].as_array() {
        for r in included {
            if let (Some(t), Some(i)) = (r["type"].as_str(), r["id"].as_str()) {
                idx.insert(format!("{t}:{i}"), r.clone());
            }
        }
    }
    idx
}

pub(crate) fn resolve<'a>(idx: &'a Index, rel: &Value) -> Option<&'a Value> {
    idx.get(&format!("{}:{}", rel["type"].as_str()?, rel["id"].as_str()?))
}

// Keep ids numeric where possible (playlist uuids stay strings).
fn id_value(raw: &Value) -> Value {
    match raw {
        Value::String(s) => match s.parse::<u64>() {
            Ok(n) => json!(n),
            Err(_) => json!(s.clone()),
        },
        other => other.clone(),
    }
}

// Artwork resources carry a `files` array of {href, meta: {width,
// height}}; the largest entry is the natural-resolution URL.
fn largest_artwork(a: &Value) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    if let Some(files) = a["attributes"]["files"].as_array() {
        for f in files {
            let (Some(href), Some(w)) = (
                f["href"].as_str(),
                f["meta"]["width"].as_u64(),
            ) else {
                continue;
            };
            if best.as_ref().map(|(w0, _)| w > *w0).unwrap_or(true) {
                best = Some((w, href.to_string()));
            }
        }
    }
    best.map(|(_, href)| href)
}

// Flatten one resource object (a `data` or `included` entry) into the
// v1-shaped JSON the mapping layer reads.
pub(crate) fn flatten_resource(obj: &Value, doc: &Value) -> Value {
    flatten_inner(obj, &index(doc))
}

// Flatten a resource using the caller-provided document index (cheaper
// than rebuilding the index per item).
pub(crate) fn flatten_with(obj: &Value, idx: &Index) -> Value {
    flatten_inner(obj, idx)
}

fn flatten_inner(obj: &Value, idx: &Index) -> Value {
    let mut v = obj["attributes"].clone();
    if let Some(id) = obj["id"].as_str() {
        v["id"] = id_value(&json!(id));
    }

    // --- relationship joins -------------------------------------------
    let Some(rels) = obj["relationships"].as_object() else {
        return normalize(obj, v);
    };

    // artists -> [{id, name}]
    if let Some(data) = rels.get("artists").and_then(|r| r["data"].as_array()) {
        let list: Vec<Value> = data
            .iter()
            .filter_map(|ident| {
                let r = resolve(idx, ident)?;
                Some(json!({
                    "id": id_value(&r["id"]),
                    "name": r["attributes"]["name"].as_str().unwrap_or(""),
                }))
            })
            .collect();
        if !list.is_empty() {
            v["artists"] = json!(list);
        }
    }

    // albums -> first album, flattened (inject its releaseDate for the
    // track year fallback).
    if let Some(first) = rels
        .get("albums")
        .and_then(|r| r["data"].as_array())
        .and_then(|data| data.first().and_then(|ident| resolve(idx, ident)))
    {
        let album = flatten_inner(first, idx);
        if album["id"].is_number() || album["id"].is_string() {
            v["album"] = album;
        }
    }

    // biographys (artist) -> text
    if let Some(rel) = rels.get("biography") {
        let ident = rel["data"].as_object().or_else(|| {
            rel["data"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|x| x.as_object())
        });
        if let Some(ident) = ident.and_then(|x| resolve(idx, &json!(x))) {
            v["text"] = ident["attributes"]["text"].clone();
        }
    }

    // genres -> genre name
    if let Some(g) = rels
        .get("genres")
        .and_then(|r| r["data"].as_array())
        .and_then(|data| {
            data.iter().find_map(|ident| {
                resolve(idx, ident).and_then(|r| {
                    r["attributes"]["genreName"].as_str().map(String::from)
                })
            })
        })
    {
        v["genre"] = json!(g);
    }

    // lyrics -> {lyrics: text, subtitles: lrcText}
    if let Some(first) = rels
        .get("lyrics")
        .and_then(|r| r["data"].as_array())
        .and_then(|data| data.first().and_then(|ident| resolve(idx, ident)))
    {
        let a = &first["attributes"];
        v["lyrics"] = a["text"].clone();
        v["subtitles"] = a["lrcText"].clone();
    }

    // coverArt / profileArt -> cover / picture (largest artwork href)
    for (rel_name, out_name) in [("coverArt", "cover"), ("profileArt", "picture")] {
        if let Some(href) = rels
            .get(rel_name)
            .and_then(|r| r["data"].as_array())
            .and_then(|data| {
                data.iter().find_map(|ident| {
                    resolve(idx, ident).and_then(largest_artwork)
                })
            })
        {
            v[out_name] = json!(href);
        }
    }

    normalize(obj, v)
}

// Rename v2 attribute names to the v1 names the mappers read.
fn normalize(obj: &Value, mut v: Value) -> Value {
    let rtype = obj["type"].as_str().unwrap_or("");
    if rtype == "albums" {
        v["type"] = v["albumType"].clone();
        v["numberOfTracks"] = v["numberOfItems"].clone();
        v["duration"] = iso_seconds(&v["duration"]);
    }
    if rtype == "playlists" {
        v["uuid"] = v["id"].clone();
        v["title"] = v["name"].clone();
        v["numberOfTracks"] = v["numberOfItems"].clone();
        v["publicPlaylist"] = json!(v["accessType"].as_str() == Some("PUBLIC"));
        v["squareImage"] = v["cover"].clone();
        v["created"] = v["createdAt"].clone();
        v["lastUpdated"] = v["lastModifiedAt"].clone();
    }
    if rtype == "tracks" {
        v["duration"] = iso_seconds(&v["duration"]);
    }
    if rtype == "artists" {
        // no attribute renames; picture comes from the profileArt join
    }
    if rtype == "users" {
        v["profileName"] = v["username"].clone();
    }
    v
}

// ISO-8601 duration ("PT4M12.345S") -> seconds as an integer.
pub(crate) fn iso_seconds(raw: &Value) -> Value {
    let Some(s) = raw.as_str() else {
        return Value::Null;
    };
    parse_iso_duration(s)
        .map(|secs| json!(secs))
        .unwrap_or(Value::Null)
}

pub(crate) fn parse_iso_duration(s: &str) -> Option<u64> {
    let body = s.strip_prefix("PT")?;
    let mut total = 0f64;
    let mut acc = String::new();
    for c in body.chars() {
        match c {
            'H' | 'M' | 'S' => {
                let unit = match c {
                    'H' => 3600.0,
                    'M' => 60.0,
                    _ => 1.0,
                };
                total += acc.parse::<f64>().ok()? * unit;
                acc.clear();
            }
            _ if c.is_ascii_digit() || c == '.' => acc.push(c),
            _ => return None,
        }
    }
    if !acc.is_empty() {
        return None;
    }
    Some(total.round() as u64)
}

// Turn a relationship-items document (the `data` array of identifiers
// plus `included` resources) into the v1 `{items: [...]}` wrapper.
// Favorites styles carry `created`; playlist styles carry a `meta`
// object with itemId so removals can address exact playlist entries.
pub(crate) fn flatten_item_entries(doc: &Value, with_meta: bool) -> Value {
    let idx = index(doc);
    let Some(idents) = doc["data"].as_array() else {
        return json!({ "items": [] });
    };
    let mut items = Vec::with_capacity(idents.len());
    for ident in idents {
        let (Some(rtype), Some(id)) = (ident["type"].as_str(), ident["id"].as_str()) else {
            continue;
        };
        // The resource may be embedded in `included`; if absent, keep a
        // minimal placeholder so playlist indices stay stable.
        let item = match resolve(&idx, ident) {
            Some(r) => flatten_with(r, &idx),
            None => json!({ "id": id_value(&json!(id)) }),
        };
        let mut entry = json!({ "item": item, "type": rtype });
        if let Some(added) = ident["meta"]["addedAt"].as_str() {
            entry["created"] = json!(added);
        }
        if with_meta {
            let mut meta = serde_json::Map::new();
            if let Some(m) = ident["meta"].as_object() {
                for (k, val) in m {
                    match k.as_str() {
                        "trackNumber" | "volumeNumber" | "itemCursor" => {
                            meta.insert(k.clone(), val.clone());
                        }
                        _ => {}
                    }
                }
            }
            if ident["meta"]["itemId"].is_string() {
                meta.insert("itemId".to_string(), ident["meta"]["itemId"].clone());
            }
            if !meta.is_empty() {
                entry["meta"] = json!(meta);
            }
        }
        items.push(entry);
    }
    json!({ "items": items })
}

// The next-cursor for `page[cursor]` pagination, when the server links
// another page.
pub(crate) fn next_cursor(doc: &Value) -> Option<String> {
    doc["links"]["meta"]["nextCursor"]
        .as_str()
        .map(String::from)
}

// A /searchResults document into the v1 search shape: each section's
// items come from the resource's relationship identifiers in order.
// Scanning `included` by type instead would mix categories.
pub(crate) fn flatten_search(doc: &Value) -> Value {
    let idx = index(doc);
    // data is the searchResults documents array; the resource links live
    // on its first element.
    let rels = doc["data"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r["relationships"].as_object());
    let mut out = serde_json::Map::new();
    for (category, key) in [
        ("albums", "albums"),
        ("artists", "artists"),
        ("tracks", "tracks"),
        ("playlists", "playlists"),
    ] {
        let mut items: Vec<Value> = Vec::new();
        if let Some(data) = rels
            .and_then(|r| r.get(category))
            .and_then(|rel| rel["data"].as_array())
        {
            for ident in data {
                if let Some(r) = resolve(&idx, ident) {
                    items.push(flatten_with(r, &idx));
                }
            }
        }
        out.insert(key.to_string(), json!({ "items": items }));
    }
    Value::Object(out)
}

// Relationship-items documents where the v1 shape was a bare list of
// resources (album tracks, artist top tracks): return each flattened
// resource directly, with trackNumber/volumeNumber injected from the
// item identifier's meta (the only place v2 carries them).
pub(crate) fn bare_items(doc: &Value) -> Vec<Value> {
    let idx = index(doc);
    let Some(idents) = doc["data"].as_array() else {
        return Vec::new();
    };
    idents
        .iter()
        .filter_map(|ident| {
            let mut item = match resolve(&idx, ident) {
                Some(r) => flatten_with(r, &idx),
                None => return None,
            };
            let meta = &ident["meta"];
            if meta["trackNumber"].is_number() {
                item["trackNumber"] = meta["trackNumber"].clone();
            }
            if meta["volumeNumber"].is_number() {
                item["volumeNumber"] = meta["volumeNumber"].clone();
            }
            Some(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_iso_durations() {
        assert_eq!(parse_iso_duration("PT4M12S"), Some(252));
        assert_eq!(parse_iso_duration("PT1H2M3.5S"), Some(3724));
        assert_eq!(parse_iso_duration("PT42S"), Some(42));
        assert_eq!(parse_iso_duration("4:12"), None);
        assert_eq!(parse_iso_duration(""), None);
    }

    #[test]
    fn flattens_track_with_artists_and_cover() {
        let doc = json!({
            "data": {
                "type": "tracks",
                "id": "7",
                "attributes": {
                    "title": "Run",
                    "duration": "PT4M12S",
                    "explicit": true,
                },
                "relationships": {
                    "artists": { "data": [{ "type": "artists", "id": "3" }] },
                    "albums": { "data": [{ "type": "albums", "id": "5" }] },
                    "coverArt": { "data": [{ "type": "artworks", "id": "1" }] },
                },
            },
            "included": [
                {
                    "type": "artists", "id": "3",
                    "attributes": { "name": "Ghost" },
                },
                {
                    "type": "albums", "id": "5",
                    "attributes": { "title": "Opus", "releaseDate": "2013-03-01" },
                },
                {
                    "type": "artworks", "id": "1",
                    "attributes": { "files": [
                        { "href": "https://art.tidal.com/a", "meta": { "width": 320, "height": 320 } },
                        { "href": "https://art.tidal.com/b", "meta": { "width": 1280, "height": 1280 } },
                    ]},
                },
            ],
        });
        let v = flatten_resource(&doc["data"], &doc);
        assert_eq!(v["id"], json!(7));
        assert_eq!(v["title"], "Run");
        assert_eq!(v["duration"], json!(252));
        assert_eq!(v["artists"][0]["name"], "Ghost");
        assert_eq!(v["artists"][0]["id"], json!(3));
        assert_eq!(v["album"]["title"], "Opus");
        assert_eq!(v["album"]["releaseDate"], "2013-03-01");
        assert_eq!(v["cover"], "https://art.tidal.com/b");
    }

    #[test]
    fn flattens_playlist_attributes() {
        let doc = json!({
            "data": {
                "type": "playlists",
                "id": "deadbeef-1234",
                "attributes": {
                    "name": "Late Night",
                    "description": "chill",
                    "numberOfItems": 12,
                    "accessType": "PRIVATE",
                    "createdAt": "2023-01-01T00:00:00Z",
                    "lastModifiedAt": "2023-02-01T00:00:00Z",
                    "duration": "PT48M",
                },
            },
            "included": [],
        });
        let v = flatten_resource(&doc["data"], &doc);
        assert_eq!(v["id"], "deadbeef-1234");
        assert_eq!(v["uuid"], "deadbeef-1234");
        assert_eq!(v["title"], "Late Night");
        assert_eq!(v["numberOfTracks"], 12);
        assert_eq!(v["publicPlaylist"], false);
        assert_eq!(v["created"], "2023-01-01T00:00:00Z");
        assert_eq!(v["lastUpdated"], "2023-02-01T00:00:00Z");
    }

    #[test]
    fn similar_tracks_feed_flattens_to_mappable_items() {
        // The similarTracks relationship document: identifiers in `data`,
        // full track resources in `included` (with their albums, artists,
        // and coverArt nested). Missing included resources are skipped, so
        // a partially-resolved feed degrades instead of panicking.
        let doc = json!({
            "data": [
                {"type": "tracks", "id": "11",
                 "meta": {"trackNumber": 1, "volumeNumber": 1}},
                {"type": "tracks", "id": "12"},
            ],
            "included": [
                {
                    "type": "artists", "id": "3",
                    "attributes": {"name": "Ghost"},
                },
                {
                    "type": "albums", "id": "5",
                    "attributes": {"title": "Opus", "releaseDate": "2013-03-01"},
                },
                {
                    "type": "artworks", "id": "1",
                    "attributes": {"files": [
                        {"href": "https://art.tidal.com/a",
                         "meta": {"width": 1280, "height": 1280}},
                    ]},
                },
                {
                    "type": "tracks", "id": "11",
                    "attributes": {"title": "Year Zero", "duration": "PT3M40S"},
                    "relationships": {
                        "artists": {"data": [{"type": "artists", "id": "3"}]},
                        "albums": {"data": [{"type": "albums", "id": "5"}]},
                        "coverArt": {"data": [{"type": "artworks", "id": "1"}]},
                    },
                },
            ],
        });
        let items = bare_items(&doc);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "Year Zero");
        assert_eq!(items[0]["duration"], json!(220));
        assert_eq!(items[0]["album"]["id"], json!(5));
        assert_eq!(items[0]["album"]["title"], "Opus");
        assert_eq!(items[0]["artists"][0]["name"], "Ghost");
        assert_eq!(items[0]["cover"], "https://art.tidal.com/a");
        assert_eq!(items[0]["trackNumber"], 1);
        assert_eq!(items[0]["volumeNumber"], 1);
    }

    #[test]
    fn search_results_links_resolve_from_the_results_document() {
        let doc = json!({
            "data": [{
                "type": "searchResults",
                "id": "search",
                "relationships": {
                    "artists": {
                        "data": [
                            {"type": "artists", "id": "5"}
                        ]
                    }
                }
            }],
            "included": [
                {"type": "artists", "id": "5",
                 "attributes": {"name": "Green Day"}}
            ]
        });
        let v = flatten_search(&doc);
        assert_eq!(v["artists"]["items"][0]["name"], "Green Day");
        assert_eq!(v["tracks"]["items"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn item_entries_with_meta() {
        let doc = json!({
            "data": [
                { "type": "tracks", "id": "7",
                  "meta": { "itemId": "aa", "trackNumber": 2, "addedAt": "2023-01-01" } },
                { "type": "videos", "id": "8" },
            ],
            "included": [
                { "type": "tracks", "id": "7",
                  "attributes": { "title": "Run", "duration": "PT4M" } },
            ],
        });
        let v = flatten_item_entries(&doc, true);
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
        assert_eq!(v["items"][0]["item"]["title"], "Run");
        assert_eq!(v["items"][0]["meta"]["itemId"], "aa");
        assert_eq!(v["items"][0]["meta"]["trackNumber"], 2);
        assert_eq!(v["items"][0]["created"], "2023-01-01");
        assert_eq!(v["items"][1]["type"], "videos");
        assert_eq!(v["items"][1]["item"]["id"], json!(8));
    }
}