// Subsonic response models, one module per entity family. Each endpoint's
// response struct sits next to its payload types.
pub mod album;
pub mod artist;
pub mod favorites;
pub mod playlist;
pub mod search;
pub mod song;
pub mod system;

pub use album::{
    AlbumId3, AlbumList2, AlbumList2Response, AlbumWithSongs, GetAlbumResponse,
};
pub use artist::{
    ArtistId3, ArtistInfo2, ArtistInfo2Response, ArtistWithAlbums, GetArtistResponse,
};
pub use favorites::{
    Starred, Starred2, Starred2Album, Starred2Artist, Starred2Response, StarredAlbum,
    StarredArtist, StarredResponse,
};
pub use playlist::{GetPlaylistResponse, Playlist, Playlists, PlaylistsResponse, PlaylistWithSongs};
pub use search::{SearchResult3, SearchResult3Response};
pub use song::{Child, GetSongResponse, RandomSongs, RandomSongsResponse, TopSongs, TopSongsResponse};
pub use system::{
    GetOpenSubsonicExtensionsResponse, GetUserResponse, Genres, GenresResponse,
    JukeboxControlResponse, JukeboxPlaylist, JukeboxStatus, MusicFolder, MusicFolders,
    MusicFoldersResponse, OpenSubsonicExtension, PingResponse, ScanStatus, ScanStatusResponse,
    SubsonicBody, SubsonicError, SubsonicErrorBody, SubsonicResponse, User,
};
