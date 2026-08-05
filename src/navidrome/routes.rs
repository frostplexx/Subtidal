use super::handlers;
use super::params::QueryParams;
use warp::Filter;

// A function to build our routes
pub fn routes() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    ping()
        .or(get_user())
        .or(search3())
        .or(get_cover_art())
        .or(fallback())
        .with(warp::log("hightide"))
}

// A route to handle the OpenSubsonic ping endpoint
fn ping() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "ping")
        .or(warp::path!("rest" / "ping.view"))
        .unify()
        .and(warp::get())
        .and_then(handlers::ping)
}

fn get_user() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "getUser")
        .or(warp::path!("rest" / "getUser.view"))
        .unify()
        .and(warp::get())
        .and(warp::query::<QueryParams>())
        .and_then(handlers::get_user)
}

fn search3() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "search3")
        .or(warp::path!("rest" / "search3.view"))
        .unify()
        .and(warp::get())
        .and(warp::query::<QueryParams>())
        .and_then(handlers::search3)
}

fn get_cover_art() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "getCoverArt")
        .or(warp::path!("rest" / "getCoverArt.view"))
        .unify()
        .and(warp::get())
        .and(warp::query::<QueryParams>())
        .and_then(handlers::get_cover_art)
}

// Last resort: log any /rest/ endpoint the server does not map yet.
// Keeps 404 as the status; the warning shows what clients are asking for.
fn fallback() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::any().and(warp::path::tail()).and_then(|tail: warp::path::Tail| async move {
        let path = format!("/{}", tail.as_str());
        if path.starts_with("/rest/") {
            tracing::warn!("unmapped endpoint: {path}");
        } else {
            tracing::debug!("unmapped path: {path}");
        }
        let reply = warp::reply::with_status(warp::reply(), warp::http::StatusCode::NOT_FOUND);
        Ok::<_, warp::Rejection>(reply)
    })
}
