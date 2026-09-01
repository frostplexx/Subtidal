// Library browsing: getIndexes, getArtists, and getMusicDirectory. The
// library is the favorited-artist list; directories hang off it:
//   folder 1  -> favorited artists
//   ar<id>    -> that artist's albums
//   al<id>    -> that album's tracks
use std::collections::BTreeMap;

use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::{
    Artists, ArtistsResponse, Directory, DirectoryChild, DirectoryResponse, IndexArtist,
    IndexGroup, Indexes, IndexesResponse,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::client::Error;
use crate::tidal::mapping::{album_from_tidal, artist_pic_url, song_from_track, year_from};

// Leading articles the index strips before bucketing (Navidrome's default
// list). Serve the same string in ignoredArticles so clients know the rule.
const IGNORED_ARTICLES: &str = "The El La Los Las Le Les";

// The whole favorited-artist list, sorted by index key. Each favorites
// entry wraps the artist in { item, created }; created is the favorite
// time, which getIndexes reports as starred.
async fn favorite_artists() -> Result<Vec<IndexArtist>, ()> {
    let result = match crate::tidal::client().favorite_artists(0, 2000).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal favorites fetch failed: {e}");
            return Err(());
        }
    };
    let mut artists: Vec<IndexArtist> = result["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let item = &entry["item"];
                    let id = item["id"].as_u64()?;
                    let name = item["name"].as_str()?.to_string();
                    Some(IndexArtist {
                        id: ids::encode_artist(id),
                        name,
                        cover_art: item["picture"].as_str().map(|p| artist_pic_url(p, 480)),
                        album_count: item["albumCount"].as_u64().map(|n| n as u32),
                        starred: entry["created"].as_str().map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    artists.sort_by(|a, b| {
        (sort_key(&a.name), a.name.to_lowercase()).cmp(&(sort_key(&b.name), b.name.to_lowercase()))
    });
    Ok(artists)
}

// Shared index build for getIndexes and getArtists. Returns the letter
// groups and the newest favorite time (the lastModified stamp).
async fn index_core(q: &QueryParams) -> Result<(Vec<IndexGroup>, i64), &'static str> {
    // Only folder 1 exists; any other id yields an empty index.
    if let Some(folder) = q.music_folder_id
        && folder != 1 {
            return Ok((Vec::new(), 0));
        }
    let artists = match favorite_artists().await {
        Ok(v) => v,
        Err(()) => return Err("Artist index unavailable"),
    };
    let last_modified = last_modified(&artists);
    Ok((index_groups(artists), last_modified))
}

// getIndexes: the classic artist index. ifModifiedSince is accepted and
// ignored; the list always reflects the current favorites.
pub async fn get_indexes(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let (index, last_modified) = match index_core(&q).await {
        Ok(v) => v,
        Err(msg) => return Ok(fail(0, msg)),
    };
    Ok(ok(IndexesResponse {
        indexes: Indexes {
            ignored_articles: IGNORED_ARTICLES,
            index,
            shortcut: Vec::new(),
            child: Vec::new(),
            last_modified,
        },
    }))
}

// getArtists: the same index, ID3-style wrapper, without favorite times.
pub async fn get_artists(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let (index, _) = match index_core(&q).await {
        Ok(v) => v,
        Err(msg) => return Ok(fail(0, msg)),
    };
    let index: Vec<IndexGroup> = index
        .into_iter()
        .map(|mut g| {
            for artist in &mut g.artist {
                artist.starred = None;
            }
            g
        })
        .collect();
    Ok(ok(ArtistsResponse {
        artists: Artists {
            ignored_articles: IGNORED_ARTICLES,
            index,
        },
    }))
}

// The index key: the name without a leading article, lowercased.
// "The Beatles" sorts as "beatles"; a bare article sorts as "".
fn sort_key(name: &str) -> String {
    let lower = name.to_lowercase();
    for article in IGNORED_ARTICLES.split(' ') {
        let a = article.to_lowercase();
        if lower == a {
            return String::new();
        }
        if let Some(rest) = lower.strip_prefix(&format!("{a} ")) {
            return rest.to_string();
        }
    }
    lower
}

// The bucket letter: the first alphabetic character of the index key,
// uppercase; anything else lands in "#".
fn index_letter(name: &str) -> String {
    match sort_key(name).chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        _ => "#".to_string(),
    }
}

// Bucket the sorted artist list into letter groups, in letter order.
fn index_groups(artists: Vec<IndexArtist>) -> Vec<IndexGroup> {
    let mut groups: BTreeMap<String, Vec<IndexArtist>> = BTreeMap::new();
    for a in artists {
        groups.entry(index_letter(&a.name)).or_default().push(a);
    }
    groups
        .into_iter()
        .map(|(name, artist)| IndexGroup { name, artist })
        .collect()
}

// The newest favorite time in epoch ms; the index's lastModified stamp.
// Falls back to the current time when no favorite carries a date.
fn last_modified(artists: &[IndexArtist]) -> i64 {
    artists
        .iter()
        .filter_map(|a| a.starred.as_deref().and_then(iso8601_ms))
        .max()
        .unwrap_or_else(now_ms)
}

// Tidal favorite times look like "2023-01-15T10:00:00.000Z". Anything
// else yields None. Only the Z suffix is handled; Tidal always sends it.
fn iso8601_ms(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let time = time.strip_suffix('Z').unwrap_or(time);
    let (hms, _) = time.split_once('.').unwrap_or((time, ""));
    let (y, m, d) = date_parts(date)?;
    let (h, mi, sec) = hms_parts(hms)?;
    let days = days_from_civil(y, m, d);
    Some((days * 86_400 + h as i64 * 3_600 + mi as i64 * 60 + sec as i64) * 1_000)
}

fn date_parts(s: &str) -> Option<(i64, u32, u32)> {
    let mut it = s.split('-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((y, m, d))
}

fn hms_parts(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.split(':');
    let h = it.next()?.parse().ok()?;
    let mi = it.next()?.parse().ok()?;
    let sec = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((h, mi, sec))
}

// Days since the epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// getMusicDirectory: one level of the library tree. The id comes from
// getIndexes, getArtists, or a previous directory listing; it may be the
// root folder, an artist, or an album. Bare numbers are legacy client
// ids: artists are the common case (they come from the index), albums
// are the fallback.
pub async fn get_music_directory(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let client = crate::tidal::client();
    let dir = match ids::parse(id) {
        Some((IdKind::Artist, artist_id)) => match artist_directory(client, artist_id).await {
            Ok(d) => d,
            Err(e) => return Ok(directory_error(e)),
        },
        Some((IdKind::Album, album_id)) => match album_directory(client, album_id).await {
            Ok(d) => d,
            Err(e) => return Ok(directory_error(e)),
        },
        // Track and playlist ids are not directories.
        Some(_) => return Ok(fail(70, "Directory not found")),
        // The single music folder, id 1 from getMusicFolders.
        None if id == "1" => match root_directory(client).await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("tidal directory fetch failed: {e}");
                return Ok(fail(0, "Directory unavailable"));
            }
        },
        None => match id.parse::<u64>() {
            Ok(n) => match artist_directory(client, n).await {
                Ok(d) => d,
                Err(_) => match album_directory(client, n).await {
                    Ok(d) => d,
                    Err(e) => return Ok(directory_error(e)),
                },
            },
            Err(_) => return Ok(fail(70, "Directory not found")),
        },
    };
    Ok(ok(DirectoryResponse { directory: dir }))
}

// Map a Tidal error to a Subsonic failure reply.
fn directory_error(e: Error) -> warp::reply::Json {
    match e {
        Error::Tidal(404, _) => fail(70, "Directory not found"),
        e => {
            tracing::error!("tidal directory fetch failed: {e}");
            fail(0, "Directory unavailable")
        }
    }
}

// The root folder (id 1): favorited artists as subdirectories.
async fn root_directory(client: &crate::tidal::client::TidalClient) -> Result<Directory, Error> {
    let result = client.favorite_artists(0, 2000).await?;
    let child: Vec<DirectoryChild> = result["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let item = &entry["item"];
                    let id = item["id"].as_u64()?;
                    let name = item["name"].as_str()?.to_string();
                    Some(dir_entry(
                        ids::encode_artist(id),
                        "1".to_string(),
                        name.clone(),
                        name.clone(),
                        name,
                        item["picture"].as_str().map(|p| artist_pic_url(p, 480)),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Directory {
        id: "1".to_string(),
        name: "Tidal".to_string(),
        child,
    })
}

// An artist directory: the artist's albums as subdirectories. The artist
// detail fetch also validates the id and supplies the directory name.
async fn artist_directory(
    client: &crate::tidal::client::TidalClient,
    artist_id: u64,
) -> Result<Directory, Error> {
    let detail = client.artist(artist_id).await?;
    let albums = client.artist_albums(artist_id).await?;
    let child: Vec<DirectoryChild> = albums["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| {
                    let a = album_from_tidal(v)?;
                    let mut e = dir_entry(
                        a.id.clone(),
                        ids::encode_artist(artist_id),
                        a.name.clone(),
                        a.name.clone(),
                        a.artist.clone(),
                        a.cover_art.clone(),
                    );
                    e.year = a.year;
                    e.duration = a.duration;
                    e.song_count = a.song_count;
                    Some(e)
                })
                .collect()
        })
        .unwrap_or_default();
    let name = detail["name"].as_str().unwrap_or("").to_string();
    Ok(Directory {
        id: ids::encode_artist(artist_id),
        name,
        child,
    })
}

// An album directory: the album's tracks, in track order. The album year
// fills in for tracks that carry no release date. 
async fn album_directory(
    client: &crate::tidal::client::TidalClient,
    album_id: u64,
) -> Result<Directory, Error> {
    let detail_v1 = match client.album_v1(album_id).await {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::debug!("v1 album detail failed, falling back to v2: {e}");
            None
        }
    };
    let tracks_v1 = match &detail_v1 {
        Some(_) => match client.album_tracks_v1(album_id).await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::debug!("v1 album tracks failed, falling back to v2: {e}");
                None
            }
        },
        None => None,
    };
    let (detail, tracks) = match (detail_v1, tracks_v1) {
        (Some(d), Some(t)) => (d, t["items"].as_array().cloned().unwrap_or_default()),
        _ => match client.album_with_items(album_id).await {
            Ok(v) => (
                v["album"].clone(),
                v["items"].as_array().cloned().unwrap_or_default(),
            ),
            Err(e) => {
                tracing::error!("tidal album fetch failed: {e}");
                return Err(e);
            }
        },
    };
    let year = year_from(detail["releaseDate"].as_str());
    let mut child: Vec<DirectoryChild> = tracks
        .iter()
        .filter_map(|t| song_from_track(t).map(DirectoryChild::from))
        .collect();
    for c in &mut child {
        if c.year.is_none() {
            c.year = year;
        }
    }
    let name = detail["title"].as_str().unwrap_or("").to_string();
    Ok(Directory {
        id: ids::encode_album(album_id),
        name,
        child,
    })
}

// One minimal directory entry for a subdirectory.
fn dir_entry(
    id: String,
    parent: String,
    title: String,
    album: String,
    artist: String,
    cover_art: Option<String>,
) -> DirectoryChild {
    DirectoryChild {
        id,
        parent,
        is_dir: true,
        is_video: false,
        title,
        album,
        artist,
        track: None,
        year: None,
        genre: None,
        genres: None,
        explicit_status: None,
        cover_art,
        duration: None,
        disc_number: None,
        album_id: None,
        artist_id: None,
        kind: None,
        content_type: None,
        suffix: None,
        size: None,
        path: None,
        created: None,
        song_count: None,
        replay_gain: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artist(id: u64, name: &str, starred: &str) -> IndexArtist {
        IndexArtist {
            id: ids::encode_artist(id),
            name: name.to_string(),
            cover_art: None,
            album_count: None,
            starred: Some(starred.to_string()),
        }
    }

    #[test]
    fn index_groups_by_stripped_letter() {
        let artists = vec![
            artist(1, "The Beatles", "2023-01-01T00:00:00.000Z"),
            artist(2, "abba", "2023-01-02T00:00:00.000Z"),
            artist(3, "Alphaville", "2023-01-03T00:00:00.000Z"),
            artist(4, "2Pac", "2023-01-04T00:00:00.000Z"),
        ];
        let groups = index_groups(artists);
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["#", "A", "B"]);
        assert_eq!(groups[1].artist[0].name, "abba");
        assert_eq!(groups[1].artist[1].name, "Alphaville");
        assert_eq!(groups[2].artist[0].name, "The Beatles");
    }

    #[test]
    fn index_letter_handles_edges() {
        assert_eq!(index_letter("zz top"), "Z");
        assert_eq!(index_letter("The The"), "T");
        assert_eq!(index_letter("_underscore"), "#");
        assert_eq!(index_letter("El"), "#");
    }

    #[test]
    fn iso8601_ms_parses_tidal_timestamps() {
        assert_eq!(iso8601_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(iso8601_ms("2023-01-15T10:00:00.000Z"), Some(1_673_776_800_000));
        assert_eq!(iso8601_ms("2023-01-15T10:00:00Z"), Some(1_673_776_800_000));
        assert_eq!(iso8601_ms("garbage"), None);
    }

    #[test]
    fn last_modified_takes_newest_favorite() {
        let artists = vec![
            artist(1, "A", "2023-01-01T00:00:00.000Z"),
            artist(2, "B", "2023-01-02T00:00:00.000Z"),
        ];
        assert_eq!(last_modified(&artists), 1_672_617_600_000);
        let before = now_ms();
        let v = last_modified(&[]);
        let after = now_ms();
        assert!(before <= v && v <= after);
    }
}
