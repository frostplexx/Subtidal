// Subsonic response models, one module per entity family. Each endpoint's
// response struct sits next to its payload types.
pub mod album;
pub mod artist;
pub mod browse;
pub mod favorites;
pub mod misc;
pub mod playlist;
pub mod queue;
pub mod search;
pub mod song;
pub mod system;
pub mod transcode;

pub use album::{
    Album, AlbumId3, AlbumInfo, AlbumInfo2Response, AlbumInfoResponse, AlbumList,
    AlbumList2, AlbumList2Response, AlbumListResponse, AlbumWithSongs, GetAlbumResponse,
};
pub use artist::{
    Artist, ArtistId3, ArtistInfo2, ArtistInfo2Response, ArtistInfoResponse, ArtistWithAlbums,
    GetArtistResponse,
};
pub use browse::{
    Artists, ArtistsResponse, Directory, DirectoryChild, DirectoryResponse, IndexArtist,
    IndexGroup, Indexes, IndexesResponse,
};
pub use favorites::{
    Starred, Starred2, Starred2Album, Starred2Artist, Starred2Response, StarredAlbum,
    StarredArtist, StarredResponse,
};
pub use misc::{
    InternetRadioStations, InternetRadioStationsResponse, Shares, SharesResponse,
};
pub use playlist::{GetPlaylistResponse, Playlist, Playlists, PlaylistsResponse, PlaylistWithSongs};
pub use queue::{Bookmark, Bookmarks, BookmarksResponse, PlayQueue, PlayQueueByIndex, PlayQueueByIndexResponse, PlayQueueResponse};
pub use search::{SearchResult2, SearchResult2Response, SearchResult3, SearchResult3Response};
pub use song::{
    char_range_to_byte_range, Child, Cue, CueLine, GetSongResponse, LyricAgent, LyricLine, Lyrics,
    LyricsList, LyricsListResponse, LyricsResponse, NowPlaying, NowPlayingEntry,
    NowPlayingResponse, RandomSongs, RandomSongsResponse, ReplayGain, SimilarSongs, SimilarSongs2,
    SimilarSongs2Response, SimilarSongsResponse, SongsByGenre, SongsByGenreResponse,
    StructuredLyrics, TopSongs, TopSongsResponse,
};
pub use system::{
    GetOpenSubsonicExtensionsResponse, GetUserResponse, GetUsersResponse, Genres, GenresResponse,
    JukeboxControlResponse, JukeboxPlaylist, JukeboxStatus, License, LicenseResponse, MusicFolder,
    MusicFolders, MusicFoldersResponse, OpenSubsonicExtension, PingResponse, ScanStatus,
    ScanStatusResponse, SubsonicBody, SubsonicError, SubsonicErrorBody, SubsonicResponse, User,
    Users,
};
pub use transcode::{StreamDetails, TranscodeDecision, TranscodeDecisionResponse};
