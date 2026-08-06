use super::handlers;
use super::log::{logged, named};
use super::params::QueryParams;
use warp::Filter;

// A function to build our routes
pub fn routes() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    logged(
        ping()
            .or(get_user())
            .or(search3())
            .or(get_cover_art())
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

fn get_user() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "getUser")
        .or(warp::path!("rest" / "getUser.view"))
        .unify()
        .and(warp::get())
        .and(warp::query::<QueryParams>())
        .and_then(handlers::get_user)
        .map(named("getUser"))
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
