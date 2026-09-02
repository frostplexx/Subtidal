// Tidal play queue mirror: disabled. The feature is gated behind an
// experiment flag that cannot be enabled on the API side. The queue
// stays in the local play_state store. Re-enable by removing the
// comment wrapper below.
/*
// Play queue sync. Tidal keeps one queue per user, populated through
// relationship mutations. A Subsonic savePlayQueue maps onto one
// future-relationship call with mode REPLACE_ALL_AND_CURRENT: data[0]
// becomes the current track, the rest the future list. Tidal's past
// relationship is not writable over the public API, so the tracks before
// the current one stay in the local store only. The queue id is
// discovered by owner and cached in memory; a restart re-discovers it.

use serde_json::{json, Value};

use super::{jsonapi, Error, TidalClient};

// Mirror at most this many tracks. Tidal's own players cap their queues
// near this size; longer queues stay local-only.
#[cfg(not(test))]
const QUEUE_SYNC_CAP: usize = 1000;
// Read cap for a long restored queue.
const QUEUE_READ_CAP: usize = 2000;

impl TidalClient {
    // The user's play queue id, creating the queue when none exists.
    #[cfg(not(test))]
    pub async fn play_queue_id(&self) -> Result<String, Error> {
        if let Some(id) = self.queue_id.lock().await.as_ref() {
            return Ok(id.clone());
        }
        let doc = self
            .openapi_get(
                "/playQueues",
                &[("filter[owners.id]", "me")],
                &self.queue_cache,
            )
            .await?;
        let id = match doc["data"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|q| q["id"].as_str())
        {
            Some(id) => id.to_string(),
            None => {
                let created = self
                    .openapi_send(
                        reqwest::Method::POST,
                        "/playQueues",
                        Some(&json!({ "data": { "type": "playQueues" } })),
                    )
                    .await?;
                created["data"]["id"].as_str().map(String::from).ok_or_else(|| {
                    Error::Tidal(502, "playQueue create returned no id".to_string())
                })?
            }
        };
        *self.queue_id.lock().await = Some(id.clone());
        Ok(id)
    }

    // The user's play queue id without creating anything; None when the
    // account has no queue.
    pub async fn find_play_queue_id(&self) -> Result<Option<String>, Error> {
        if let Some(id) = self.queue_id.lock().await.as_ref() {
            return Ok(Some(id.clone()));
        }
        let doc = self
            .openapi_get(
                "/playQueues",
                &[("filter[owners.id]", "me")],
                &self.queue_cache,
            )
            .await?;
        let id = doc["data"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|q| q["id"].as_str())
            .map(String::from);
        if let Some(id) = &id {
            *self.queue_id.lock().await = Some(id.clone());
        }
        Ok(id)
    }

    // Replace the queue on Tidal: data[0] becomes the current track,
    // followed by the tracks after it in the saved queue.
    #[cfg(not(test))]
    pub async fn push_play_queue(&self, ids: &[u64], current: u64) -> Result<(), Error> {
        let Some(data) = queue_items(ids, current, QUEUE_SYNC_CAP) else {
            return Err(Error::Tidal(
                422,
                "current track is not in the saved queue".to_string(),
            ));
        };
        let id = self.play_queue_id().await?;
        let payload = json!({ "data": data, "meta": { "mode": "REPLACE_ALL_AND_CURRENT" } });
        self.openapi_send(
            reqwest::Method::POST,
            &format!("/playQueues/{id}/relationships/future"),
            Some(&payload),
        )
        .await?;
        self.drop_queue_cache();
        Ok(())
    }

    // Remove the queue on Tidal when an empty save clears it.
    #[cfg(not(test))]
    pub async fn clear_play_queue(&self) -> Result<(), Error> {
        let id = match self.queue_id.lock().await.clone() {
            Some(id) => id,
            None => match self.find_play_queue_id().await? {
                Some(id) => id,
                None => return Ok(()),
            },
        };
        self.openapi_send(reqwest::Method::DELETE, &format!("/playQueues/{id}"), None)
            .await?;
        *self.queue_id.lock().await = None;
        self.drop_queue_cache();
        Ok(())
    }

    // The saved queue as ordered track ids, the current track between
    // past and future. Tracks are read from the three relationships, so
    // the order matches what mobile clients show.
    pub async fn fetch_play_queue(&self) -> Result<(Vec<u64>, Option<u64>), Error> {
        let Some(id) = self.find_play_queue_id().await? else {
            return Ok((Vec::new(), None));
        };
        let past = self.relationship_track_ids(&id, "past").await?;
        let current = self.current_track_id(&id).await?;
        let future = self.relationship_track_ids(&id, "future").await?;
        let mut ids = past;
        if let Some(c) = current {
            ids.push(c);
        }
        ids.extend(future);
        Ok((ids, current))
    }

    // Walk one cursor-paged relationship (future, past) collecting track
    // ids in order.
    async fn relationship_track_ids(&self, queue_id: &str, rel: &str) -> Result<Vec<u64>, Error> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut path = format!("/playQueues/{queue_id}/relationships/{rel}");
            if let Some(c) = &cursor {
                path.push_str(&format!("?page[cursor]={c}"));
            }
            let doc = self.openapi_get_raw(&path, &self.queue_cache).await?;
            for e in doc["data"].as_array().cloned().unwrap_or_default() {
                if e["type"].as_str() != Some("tracks") {
                    continue;
                }
                if let Some(t) = e["id"].as_str().and_then(|s| s.parse::<u64>().ok()) {
                    out.push(t);
                }
            }
            match jsonapi::next_cursor(&doc) {
                Some(c) => cursor = Some(c),
                None => break,
            }
            if out.len() >= QUEUE_READ_CAP {
                break;
            }
        }
        Ok(out)
    }

    // The current relationship is a to-one document.
    async fn current_track_id(&self, queue_id: &str) -> Result<Option<u64>, Error> {
        let doc = self
            .openapi_get(
                &format!("/playQueues/{queue_id}/relationships/current"),
                &[],
                &self.queue_cache,
            )
            .await?;
        Ok(doc["data"]["id"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok()))
    }

    #[cfg(not(test))]
    fn drop_queue_cache(&self) {
        let _ = self
            .queue_cache
            .invalidate_entries_if(|k, _| k.starts_with("/playQueues"));
    }
}

// The future-relationship payload items for one Subsonic queue: the
// current track first (REPLACE_ALL_AND_CURRENT promotes data[0] to
// current), then the tracks that follow it, capped. None when the
// current track is not in the queue.
fn queue_items(ids: &[u64], current: u64, cap: usize) -> Option<Vec<Value>> {
    let pos = ids.iter().position(|t| *t == current)?;
    Some(
        ids[pos..]
            .iter()
            .take(cap)
            .map(|t| json!({ "id": t.to_string(), "type": "tracks" }))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_items_puts_current_first_then_followers() {
        let items = queue_items(&[1, 2, 99, 4], 2, 100).unwrap();
        let ids: Vec<String> = items
            .iter()
            .map(|v| v["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["2", "99", "4"]);
        assert!(items
            .iter()
            .all(|v| v["type"].as_str() == Some("tracks")));
    }

    #[test]
    fn queue_items_caps_long_queues() {
        let ids: Vec<u64> = (0..3000).collect();
        let items = queue_items(&ids, 0, 1000).unwrap();
        assert_eq!(items.len(), 1000);
        assert_eq!(items[0]["id"], "0");
        assert_eq!(items[999]["id"], "999");
    }

    #[test]
    fn queue_items_current_at_end_keeps_just_it() {
        let items = queue_items(&[1, 2, 3], 3, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "3");
    }

    #[test]
    fn queue_items_missing_current_is_none() {
        assert!(queue_items(&[1, 2, 3], 9, 10).is_none());
        assert!(queue_items(&[], 9, 10).is_none());
    }
}
*/