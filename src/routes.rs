use super::handlers;
use warp::Filter;

// A function to build our routes
pub fn routes() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    (get_post().or(ping())).with(warp::log("hightide"))
}

// A route to handle GET requests for a specific post
fn get_post() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("posts" / u64)
        .and(warp::get())
        .and_then(handlers::get_post)
}

// A route to handle the OpenSubsonic ping endpoint
fn ping() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("rest" / "ping")
        .or(warp::path!("rest" / "ping.view"))
        .unify()
        .and(warp::get())
        .and_then(handlers::ping)
}
