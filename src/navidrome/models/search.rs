// Search response models.
use serde::Serialize;

use super::album::{Album, AlbumId3};
use super::artist::{Artist, ArtistId3};
use super::song::Child;

// search3 data: { searchResult3: { artist, album, song } }
#[derive(Serialize)]
pub struct SearchResult3Response {
    #[serde(rename = "searchResult3")]
    pub search_result: SearchResult3,
}

#[derive(Serialize)]
pub struct SearchResult3 {
    pub artist: Vec<ArtistId3>,
    pub album: Vec<AlbumId3>,
    pub song: Vec<Child>,
}

// search2 data: { searchResult2: { artist, album, song } } with the legacy
// Artist/Album shapes.
#[derive(Serialize)]
pub struct SearchResult2Response {
    #[serde(rename = "searchResult2")]
    pub search_result: SearchResult2,
}

#[derive(Serialize)]
pub struct SearchResult2 {
    pub artist: Vec<Artist>,
    pub album: Vec<Album>,
    pub song: Vec<Child>,
}
