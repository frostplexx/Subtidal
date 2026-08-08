use super::auth;
use super::handlers;
use super::log::{logged, named, with_params};
use super::params::QueryParams;
use warp::Filter;
use warp::Reply;

// A function to build our routes
pub fn routes() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    logged(
        // public() is the whitelist: only these endpoints run without
        // authentication. Keep its names out of dispatch()'s match, since
        // public wins by or-order and a duplicate arm would be dead code.
        public()
            .or(auth::require_auth().and(private()).and_then(dispatch))
            .unify()
            .recover(handlers::recover)
            .or(fallback()),
    )
}

// Public endpoints. Keep this list short: any route added here skips auth.
fn public() -> impl Filter<Extract = (warp::reply::Response,), Error = warp::Rejection> + Clone {
    ping()
        .or(get_open_subsonic_extensions())
        .unify()
        .map(|r: warp::reply::WithHeader<warp::reply::Json>| r.into_response())
}

// Private endpoints. One generic matcher covers /rest/<name> and
// /rest/<name>.view; dispatch() looks the name up and calls the handler.
// Auth runs before this matches, so unknown paths with bad credentials
// get a 40 instead of a 404.
fn private() -> impl Filter<Extract = (String,), Error = warp::Rejection> + Clone {
    warp::path("rest")
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(warp::get().or(warp::post()).unify())
        .map(|name: String| name.strip_suffix(".view").unwrap_or(&name).to_string())
}

// Calls the matched handler with the authenticated params, then tags the
// reply for the log wrapper. Unknown names reject and 404 via the fallback.
//
// How to add an endpoint:
//   1. client.rs: one get_json / get_json_q method returning Value.
//   2. params.rs: add the endpoint's query params.
//   3. mapping/ + models.rs: conversion and Subsonic response structs.
//   4. handlers.rs: compose params -> client -> mapping -> ok().
//   5. Add one arm here. The .view suffix, GET+POST and the log tag come
//      from the generic matcher. Keep names out of public().
async fn dispatch(
    q: QueryParams,
    raw: String,
    name: String,
) -> Result<warp::reply::Response, warp::Rejection> {
    let reply = match name.as_str() {
        "getUser" => handlers::get_user().await?.into_response(),
        "getUsers" => handlers::get_users().await?.into_response(),
        "getLicense" => handlers::get_license().await?.into_response(),
        "getMusicFolders" => handlers::get_music_folders().await?.into_response(),
        "getScanStatus" => handlers::get_scan_status().await?.into_response(),
        "startScan" => handlers::start_scan(q).await?.into_response(),
        "scrobble" => handlers::scrobble(q).await?.into_response(),
        "setRating" => handlers::set_rating(q).await?.into_response(),
        "search2" => handlers::search2(q).await?.into_response(),
        "search3" => handlers::search3(q).await?.into_response(),
        "getCoverArt" => handlers::get_cover_art(q).await?.into_response(),
        "getAvatar" => handlers::get_avatar(q).await?,
        "getAlbum" => handlers::get_album(q).await?.into_response(),
        "getAlbumList" => handlers::get_album_list(q).await?.into_response(),
        "getAlbumList2" => handlers::get_album_list2(q).await?.into_response(),
        "getAlbumInfo" => handlers::get_album_info(q).await?.into_response(),
        "getAlbumInfo2" => handlers::get_album_info2(q).await?.into_response(),
        "getArtist" => handlers::get_artist(q).await?.into_response(),
        "getArtistInfo" => handlers::get_artist_info(q).await?.into_response(),
        "getArtistInfo2" => handlers::get_artist_info2(q).await?.into_response(),
        "getTopSongs" => handlers::get_top_songs(q).await?.into_response(),
        "getSong" => handlers::get_song(q).await?.into_response(),
        "getSimilarSongs" => handlers::get_similar_songs(q).await?.into_response(),
        "getSimilarSongs2" => handlers::get_similar_songs2(q).await?.into_response(),
        "getSongsByGenre" => handlers::get_songs_by_genre(q).await?.into_response(),
        "getLyrics" => handlers::get_lyrics(q).await?.into_response(),
        "getLyricsBySongId" => handlers::get_lyrics_by_song_id(q).await?.into_response(),
        "getRandomSongs" => handlers::get_random_songs(q).await?.into_response(),
        "stream" => handlers::stream(q).await?,
        "updateNowPlaying" => handlers::update_now_playing(q).await?.into_response(),
        "getNowPlaying" => handlers::get_now_playing(q).await?.into_response(),
        "reportPlayback" => handlers::report_playback(q).await?.into_response(),
        "getStarred" => handlers::get_starred().await?.into_response(),
        "getStarred2" => handlers::get_starred2().await?.into_response(),
        "star" => handlers::star(q).await?.into_response(),
        "unstar" => handlers::unstar(q).await?.into_response(),
        "getGenres" => handlers::get_genres().await?.into_response(),
        "getPlaylists" => handlers::get_playlists().await?.into_response(),
        "getPlaylist" => handlers::get_playlist(q).await?.into_response(),
        "createPlaylist" => handlers::create_playlist(q).await?.into_response(),
        "updatePlaylist" => handlers::update_playlist(q).await?.into_response(),
        "deletePlaylist" => handlers::delete_playlist(q).await?.into_response(),
        "getIndexes" => handlers::get_indexes(q).await?.into_response(),
        "getArtists" => handlers::get_artists(q).await?.into_response(),
        "getMusicDirectory" => handlers::get_music_directory(q).await?.into_response(),
        "getPlayQueue" => handlers::get_play_queue(q).await?.into_response(),
        "savePlayQueue" => handlers::save_play_queue(q).await?.into_response(),
        "getBookmarks" => handlers::get_bookmarks(q).await?.into_response(),
        "createBookmark" => handlers::create_bookmark(q).await?.into_response(),
        "deleteBookmark" => handlers::delete_bookmark(q).await?.into_response(),
        "jukeboxControl" => handlers::jukebox_control(q).await?.into_response(),
        "getShares" => handlers::get_shares().await?.into_response(),
        "createShare" => handlers::create_share(q).await?.into_response(),
        "updateShare" => handlers::update_share(q).await?.into_response(),
        "deleteShare" => handlers::delete_share(q).await?.into_response(),
        "getInternetRadioStations" => handlers::get_internet_radio_stations().await?.into_response(),
        "createInternetRadioStation" => handlers::create_internet_radio_station(q).await?.into_response(),
        "updateInternetRadioStation" => handlers::update_internet_radio_station(q).await?.into_response(),
        "deleteInternetRadioStation" => handlers::delete_internet_radio_station(q).await?.into_response(),
        "download" => handlers::download(q).await?,
        _ => return Err(warp::reject::not_found()),
    };
    let reply = with_params(&raw)(reply);
    Ok(named(&name)(reply).into_response())
}

// A route to handle the OpenSubsonic ping endpoint
fn ping() -> impl Filter<Extract = (warp::reply::WithHeader<warp::reply::Json>,), Error = warp::Rejection> + Clone {
    warp::path!("rest" / "ping")
        .or(warp::path!("rest" / "ping.view"))
        .unify()
        .and(warp::get().or(warp::post()).unify())
        .and_then(handlers::ping)
        .map(named("ping"))
}

// A route to handle the OpenSubsonic getOpenSubsonicExtensions endpoint
fn get_open_subsonic_extensions()
    -> impl Filter<Extract = (warp::reply::WithHeader<warp::reply::Json>,), Error = warp::Rejection> + Clone
{
    warp::path!("rest" / "getOpenSubsonicExtensions")
        .or(warp::path!("rest" / "getOpenSubsonicExtensions.view"))
        .unify()
        .and(warp::get().or(warp::post()).unify())
        .and_then(handlers::get_open_subsonic_extensions)
        .map(named("getOpenSubsonicExtensions"))
}

// Last resort: any endpoint the server does not map yet.
// Keeps 404 as the status; the log wrapper shows what clients ask for.
fn fallback() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::any()
        .and_then(|| async move {
            let reply = warp::reply::with_status(warp::reply(), warp::http::StatusCode::NOT_FOUND);
            Ok::<_, warp::Rejection>(reply)
        })
        .map(named("fallback"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deny-by-default: the whitelist stays short and every other endpoint
    // rejects requests without credentials. SETTINGS is unset in tests, so
    // authenticate() always fails and recover maps it to a 40.
    #[tokio::test]
    async fn private_endpoints_require_auth() {
        let filter = routes();
        for path in [
            "/rest/getUser",
            "/rest/getUsers",
            "/rest/getLicense",
            "/rest/getMusicFolders",
            "/rest/getScanStatus",
            "/rest/startScan",
            "/rest/scrobble",
            "/rest/setRating",
            "/rest/search2",
            "/rest/getUser.view",
            "/rest/search3",
            "/rest/getCoverArt",
            "/rest/getAvatar",
            "/rest/getAlbum",
            "/rest/getAlbumList",
            "/rest/getAlbumList2",
            "/rest/getAlbumInfo",
            "/rest/getAlbumInfo2",
            "/rest/getArtist",
            "/rest/getArtistInfo",
            "/rest/getArtistInfo2",
            "/rest/getTopSongs",
            "/rest/getSong",
            "/rest/getSimilarSongs",
            "/rest/getSimilarSongs2",
            "/rest/getSongsByGenre",
            "/rest/getLyrics",
            "/rest/getLyricsBySongId",
            "/rest/getRandomSongs",
            "/rest/stream",
            "/rest/updateNowPlaying",
            "/rest/getNowPlaying",
            "/rest/reportPlayback",
            "/rest/getStarred",
            "/rest/getStarred2",
            "/rest/star",
            "/rest/unstar",
            "/rest/getPlaylists",
            "/rest/getPlaylist",
            "/rest/createPlaylist",
            "/rest/updatePlaylist",
            "/rest/deletePlaylist",
            "/rest/getIndexes",
            "/rest/getArtists",
            "/rest/getMusicDirectory",
            "/rest/getPlayQueue",
            "/rest/savePlayQueue",
            "/rest/getBookmarks",
            "/rest/createBookmark",
            "/rest/deleteBookmark",
            "/rest/getGenres",
            "/rest/jukeboxControl",
            "/rest/getShares",
            "/rest/createShare",
            "/rest/updateShare",
            "/rest/deleteShare",
            "/rest/getInternetRadioStations",
            "/rest/createInternetRadioStation",
            "/rest/updateInternetRadioStation",
            "/rest/deleteInternetRadioStation",
            "/rest/download",
        ] {
            let reply = warp::test::request()
                .method("GET")
                .path(path)
                .reply(&filter)
                .await;
            let body: serde_json::Value = serde_json::from_slice(reply.body()).unwrap();
            let sr = &body["subsonic-response"];
            assert_eq!(sr["status"], "failed", "{path} must require auth");
            assert_eq!(sr["error"]["code"], 40, "{path} must fail with code 40");
        }
    }

    #[tokio::test]
    async fn public_endpoints_skip_auth() {
        let filter = routes();
        for path in ["/rest/ping", "/rest/getOpenSubsonicExtensions"] {
            let reply = warp::test::request()
                .method("GET")
                .path(path)
                .reply(&filter)
                .await;
            let body: serde_json::Value = serde_json::from_slice(reply.body()).unwrap();
            let sr = &body["subsonic-response"];
            assert_eq!(sr["status"], "ok", "{path} must work without auth");
        }
    }

    // Auth runs before routing, so unknown paths are denied without
    // credentials. With valid credentials they still 404 via the fallback.
    #[tokio::test]
    async fn unknown_path_without_credentials_is_denied() {
        let reply = warp::test::request()
            .method("GET")
            .path("/rest/doesNotExist")
            .reply(&routes())
            .await;
        let body: serde_json::Value = serde_json::from_slice(reply.body()).unwrap();
        assert_eq!(body["subsonic-response"]["error"]["code"], 40);
    }

    // dispatch() rejects names no endpoint owns; the fallback turns that
    // into a 404 for authenticated requests.
    #[tokio::test]
    async fn unknown_endpoint_name_rejects() {
        let q = QueryParams::from_merged("").unwrap();
        assert!(dispatch(q, String::new(), "bogus".into()).await.is_err());
    }
}
