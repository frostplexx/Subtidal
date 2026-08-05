use super::handlers;
use super::params::QueryParams;
use warp::Filter;

// A function to build our routes
pub fn routes() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    ping().or(get_user()).with(warp::log("hightide"))
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
