use super::auth;
use super::handlers;
use super::log::{logged, named, with_params};
use super::params::QueryParams;
use bytes::Bytes;
use futures_util::TryFutureExt;
use warp::Filter;
use warp::Reply;

// A function to build our routes
pub fn routes() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    logged(
        // public() is the whitelist: only these endpoints run without
        // authentication. Keep its names out of dispatch()'s match, since
        // public wins by or-order and a duplicate arm would be dead code.
        //
        // Box the two branches: warp's or/unify/recover chain nests the
        // composed filter type and otherwise trips the compile-time
        // recursion limit. Boxing truncates the type to a flat trait object
        // at the cost of one allocation per request.
        public()
            .boxed()
            .or(auth::require_auth()
                .and(private())
                .and_then(|q: QueryParams, raw: String, body: Bytes, name: String| {
                    dispatch(q, raw, name, body)
                })
                .boxed())
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
// get a 40 instead of a 404. The request body is read once, inside
// require_auth, and passed to dispatch as raw bytes.
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
//
// dispatch is deliberately NOT async. A wrapper future containing every
// arm would give the trait solver one giant async state machine to
// normalize, which overflowed its recursion limit. Boxing each arm's
// future individually keeps every Send/TryFuture proof shallow; the
// common tail runs in one small boxed wrapper.
fn dispatch(
    q: QueryParams,
    raw: String,
    name: String,
    body: Bytes,
) -> super::handlers::BoxedTryFuture<warp::reply::Response, warp::Rejection> {
    let handler: super::handlers::BoxedTryFuture<warp::reply::Response, warp::Rejection> =
        match name.as_str() {
            "getUser" => Box::pin(handlers::get_user().map_ok(|r| r.into_response())),
            "getUsers" => Box::pin(handlers::get_users().map_ok(|r| r.into_response())),
            "getLicense" => Box::pin(handlers::get_license().map_ok(|r| r.into_response())),
            "getMusicFolders" => Box::pin(handlers::get_music_folders().map_ok(|r| r.into_response())),
            "getScanStatus" => Box::pin(handlers::get_scan_status().map_ok(|r| r.into_response())),
            "startScan" => Box::pin(handlers::start_scan(q).map_ok(|r| r.into_response())),
            "scrobble" => Box::pin(handlers::scrobble(q).map_ok(|r| r.into_response())),
            "setRating" => Box::pin(handlers::set_rating(q).map_ok(|r| r.into_response())),
            "search2" => Box::pin(handlers::search2(q).map_ok(|r| r.into_response())),
            "search3" => Box::pin(handlers::search3(q).map_ok(|r| r.into_response())),
            "getCoverArt" => Box::pin(handlers::get_cover_art(q).map_ok(|r| r.into_response())),
            "getAvatar" => Box::pin(handlers::get_avatar(q).map_ok(|r| r)),
            "getAlbum" => Box::pin(handlers::get_album(q).map_ok(|r| r.into_response())),
            "getAlbumList" => Box::pin(handlers::get_album_list(q).map_ok(|r| r.into_response())),
            "getAlbumList2" => Box::pin(handlers::get_album_list2(q).map_ok(|r| r.into_response())),
            "getAlbumInfo" => Box::pin(handlers::get_album_info(q).map_ok(|r| r.into_response())),
            "getAlbumInfo2" => Box::pin(handlers::get_album_info2(q).map_ok(|r| r.into_response())),
            "getArtist" => Box::pin(handlers::get_artist(q).map_ok(|r| r.into_response())),
            "getArtistInfo" => Box::pin(handlers::get_artist_info(q).map_ok(|r| r.into_response())),
            "getArtistInfo2" => Box::pin(handlers::get_artist_info2(q).map_ok(|r| r.into_response())),
            "getTopSongs" => Box::pin(handlers::get_top_songs(q).map_ok(|r| r.into_response())),
            "getSong" => Box::pin(handlers::get_song(q).map_ok(|r| r.into_response())),
            "getSimilarSongs" => Box::pin(handlers::get_similar_songs(q).map_ok(|r| r.into_response())),
            "getSimilarSongs2" => Box::pin(handlers::get_similar_songs2(q).map_ok(|r| r.into_response())),
            "getSongsByGenre" => Box::pin(handlers::get_songs_by_genre(q).map_ok(|r| r.into_response())),
            "getLyrics" => Box::pin(handlers::get_lyrics(q).map_ok(|r| r.into_response())),
            "getLyricsBySongId" => Box::pin(handlers::get_lyrics_by_song_id(q).map_ok(|r| r.into_response())),
            "getRandomSongs" => Box::pin(handlers::get_random_songs(q).map_ok(|r| r.into_response())),
            "stream" => Box::pin(handlers::stream(q).map_ok(|r| r)),
            "updateNowPlaying" => Box::pin(handlers::update_now_playing(q).map_ok(|r| r.into_response())),
            "getNowPlaying" => Box::pin(handlers::get_now_playing(q).map_ok(|r| r.into_response())),
            "reportPlayback" => Box::pin(handlers::report_playback(q).map_ok(|r| r.into_response())),
            "getStarred" => Box::pin(handlers::get_starred().map_ok(|r| r.into_response())),
            "getStarred2" => Box::pin(handlers::get_starred2().map_ok(|r| r.into_response())),
            "star" => Box::pin(handlers::star(q).map_ok(|r| r.into_response())),
            "unstar" => Box::pin(handlers::unstar(q).map_ok(|r| r.into_response())),
            "getGenres" => Box::pin(handlers::get_genres().map_ok(|r| r.into_response())),
            "getPlaylists" => Box::pin(handlers::get_playlists().map_ok(|r| r.into_response())),
            "getPlaylist" => Box::pin(handlers::get_playlist(q).map_ok(|r| r.into_response())),
            "createPlaylist" => Box::pin(handlers::create_playlist(q).map_ok(|r| r.into_response())),
            "updatePlaylist" => Box::pin(handlers::update_playlist(q).map_ok(|r| r.into_response())),
            "deletePlaylist" => Box::pin(handlers::delete_playlist(q).map_ok(|r| r.into_response())),
            "getIndexes" => Box::pin(handlers::get_indexes(q).map_ok(|r| r.into_response())),
            "getArtists" => Box::pin(handlers::get_artists(q).map_ok(|r| r.into_response())),
            "getMusicDirectory" => Box::pin(handlers::get_music_directory(q).map_ok(|r| r.into_response())),
            "getPlayQueue" => Box::pin(handlers::get_play_queue(q).map_ok(|r| r.into_response())),
            "savePlayQueue" => Box::pin(handlers::save_play_queue(q).map_ok(|r| r.into_response())),
            "getPlayQueueByIndex" => Box::pin(handlers::get_play_queue_by_index(q).map_ok(|r| r.into_response())),
            "savePlayQueueByIndex" => Box::pin(handlers::save_play_queue_by_index(q).map_ok(|r| r.into_response())),
            "getBookmarks" => Box::pin(handlers::get_bookmarks(q).map_ok(|r| r.into_response())),
            "createBookmark" => Box::pin(handlers::create_bookmark(q).map_ok(|r| r.into_response())),
            "deleteBookmark" => Box::pin(handlers::delete_bookmark(q).map_ok(|r| r.into_response())),
            "jukeboxControl" => Box::pin(handlers::jukebox_control(q).map_ok(|r| r.into_response())),
            "getShares" => Box::pin(handlers::get_shares().map_ok(|r| r.into_response())),
            "createShare" => Box::pin(handlers::create_share(q).map_ok(|r| r.into_response())),
            "updateShare" => Box::pin(handlers::update_share(q).map_ok(|r| r.into_response())),
            "deleteShare" => Box::pin(handlers::delete_share(q).map_ok(|r| r.into_response())),
            "getInternetRadioStations" => Box::pin(handlers::get_internet_radio_stations().map_ok(|r| r.into_response())),
            "createInternetRadioStation" => Box::pin(handlers::create_internet_radio_station(q).map_ok(|r| r.into_response())),
            "updateInternetRadioStation" => Box::pin(handlers::update_internet_radio_station(q).map_ok(|r| r.into_response())),
            "deleteInternetRadioStation" => Box::pin(handlers::delete_internet_radio_station(q).map_ok(|r| r.into_response())),
            "download" => Box::pin(handlers::download(q).map_ok(|r| r)),
            "getTranscodeDecision" => Box::pin(handlers::get_transcode_decision(q, body).map_ok(|r| r.into_response())),
            _ => return Box::pin(async move { Err(warp::reject::not_found()) }),
        };
    Box::pin(async move {
        let reply = handler.await?;
        let reply = with_params(&raw)(reply);
        Ok::<_, warp::Rejection>(named(&name)(reply).into_response())
    })
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
            "/rest/getPlayQueueByIndex",
            "/rest/savePlayQueueByIndex",
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
            "/rest/getTranscodeDecision",
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
        assert!(dispatch(q, String::new(), "bogus".into(), Bytes::new()).await.is_err());
    }

    // A POST body over the 1 MiB cap must be rejected before it is read
    // into memory, with a 413, even without valid credentials.
    #[tokio::test]
    async fn oversized_body_rejected_with_413() {
        let filter = routes();
        let big = vec![0u8; 1024 * 1024 + 1];
        let reply = warp::test::request()
            .method("POST")
            .path("/rest/getUser")
            .body(big)
            .reply(&filter)
            .await;
        assert_eq!(reply.status(), 413);
    }
}
