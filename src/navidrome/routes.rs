use super::handlers;
use super::log::{logged, named, with_params};
use super::params::QueryParams;
use warp::Filter;

// A function to build our routes
pub fn routes() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    logged(
        ping()
            .or(get_open_subsonic_extensions())
            .or(get_user())
            .or(search3())
            .or(get_cover_art())
            // .or(get_album_list2())
            .or(fallback()),
    )
}

// A route to handle the OpenSubsonic ping endpoint
fn ping() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "ping")
        .or(warp::path!("rest" / "ping.view"))
        .unify()
        .and(warp::get())
        .and_then(handlers::ping)
        .map(named("ping"))
}

// fn get_album_list2() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {    warp::path!("rest" / "getAlbumList2")
//         .or(warp::path!("rest" / "getAlbumList2.view"))
//         .unify()
//         .and(warp::get())
//         // .and(warp::query::<QueryParams>())
//         .and_then(handlers::get_album_list2)
//         .map(named("getAlbumList2"))
// }

// A route to handle the OpenSubsonic getOpenSubsonicExtensions endpoint
fn get_open_subsonic_extensions()
    -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone
{
    warp::path!("rest" / "getOpenSubsonicExtensions")
        .or(warp::path!("rest" / "getOpenSubsonicExtensions.view"))
        .unify()
        .and(warp::get())
        .and_then(handlers::get_open_subsonic_extensions)
        .map(named("getOpenSubsonicExtensions"))
}

fn get_user() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "getUser")
        .or(warp::path!("rest" / "getUser.view"))
        .unify()
        .and(warp::get().or(warp::post()).unify())
        .and(subsonic_params())
        .and_then(|q: QueryParams, raw: String| async move {
            Ok::<_, warp::Rejection>((handlers::get_user(q).await?, raw))
        })
        .map(|(reply, raw): (warp::reply::Json, String)| with_params(&raw)(reply))
        .map(named("getUser"))
}

// Subsonic clients send params either in the URL (GET) or as a
// form-encoded body (POST, the OpenSubsonic formPost extension).
// Extracts the merged params plus the raw merged string for logging.
fn subsonic_params()
    -> impl Filter<Extract = (QueryParams, String), Error = warp::Rejection> + Clone
{
    warp::query::raw()
        .or_else(|_| async { Ok::<_, std::convert::Infallible>((String::new(),)) })
        .and(warp::body::bytes())
        .and_then(|query: String, body: warp::hyper::body::Bytes| async move {
            let merged = QueryParams::merge_raw(&query, &body);
            let params = QueryParams::from_merged(&merged).map_err(|_| warp::reject::reject())?;
            Ok::<_, warp::Rejection>((params, merged))
        })
        .untuple_one()
}

fn search3() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "search3")
        .or(warp::path!("rest" / "search3.view"))
        .unify()
        .and(warp::get())
        .and(warp::query::<QueryParams>())
        .and_then(handlers::search3)
        .map(named("search3"))
}

fn get_cover_art() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "getCoverArt")
        .or(warp::path!("rest" / "getCoverArt.view"))
        .unify()
        .and(warp::get())
        .and(warp::query::<QueryParams>())
        .and_then(handlers::get_cover_art)
        .map(named("getCoverArt"))
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
