// Search response models.
use serde::Serialize;

use super::album::AlbumId3;
use super::artist::ArtistId3;
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
