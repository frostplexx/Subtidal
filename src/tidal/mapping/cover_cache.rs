// Maps an opaque coverArt id (al<id>, ar<id>, a playlist UUID, mx<id>) to
// the raw Tidal cover value list responses already carried (a cover/
// picture UUID, or a full image URL for mixes). getCoverArt checks this
// before falling back to a per-item detail fetch, so browsing a list of
// albums/artists doesn't cost one extra Tidal request per cover.
use std::sync::LazyLock;
use std::time::Duration;

use moka::sync::Cache;

static COVER_CACHE: LazyLock<Cache<String, String>> = LazyLock::new(|| {
    Cache::builder()
        .time_to_live(Duration::from_secs(300))
        .max_capacity(10_000)
        .build()
});

pub(crate) fn remember(id: &str, raw_cover: &str) {
    COVER_CACHE.insert(id.to_string(), raw_cover.to_string());
}

pub(crate) fn lookup(id: &str) -> Option<String> {
    COVER_CACHE.get(id)
}
