use std::net::SocketAddr;
use std::time::Instant;

use warp::filters::path::FullPath;
use warp::http::{Method, StatusCode};
use warp::reply::{Reply, WithHeader};
use warp::Filter;

const ROUTE_HEADER: &str = "x-subtidal-route";

// Returns a closure that tags a reply with the route's endpoint name.
// Apply it with `.map(named("search3"))` inside each route function.
// The wrapper in `logged` reads and strips this header, so clients never see it.
pub fn named<R: Reply>(name: &'static str) -> impl Fn(R) -> WithHeader<R> + Clone {
    move |reply| warp::reply::with_header(reply, ROUTE_HEADER, name)
}

// Wrap the whole route chain. Wrap it once at the end, not per route:
// per-route wrapping logs a 404 for every route that rejects before a match.
pub fn logged<R>(
    filter: impl Filter<Extract = (R,), Error = warp::Rejection> + Clone + Send,
) -> impl Filter<Extract = (impl Reply,), Error = warp::Rejection> + Clone + Send
where
    R: Reply,
{
    warp::any()
        .map(Instant::now)
        .and(warp::path::full())
        .and(warp::method())
        .and(warp::addr::remote())
        .and(filter)
        .map(
            |start: Instant,
             full: FullPath,
             method: Method,
             remote: Option<SocketAddr>,
             reply: R| {
                let mut resp = reply.into_response();
                let endpoint = resp
                    .headers_mut()
                    .remove(ROUTE_HEADER)
                    .and_then(|v| v.to_str().ok().map(str::to_owned));
                log_request(
                    start,
                    &method,
                    full.as_str(),
                    remote,
                    resp.status(),
                    endpoint.as_deref(),
                );
                resp
            },
        )
}

fn log_request(
    start: Instant,
    method: &Method,
    path: &str,
    remote: Option<SocketAddr>,
    status: StatusCode,
    endpoint: Option<&str>,
) {
    let endpoint = endpoint.unwrap_or("-");
    let remote = remote.map(|a| a.to_string()).unwrap_or_else(|| "-".into());
    let elapsed = start.elapsed();
    let line = format!("{remote} {endpoint} \"{method} {path}\" {} in {elapsed:?}", status.as_u16());
    if status.is_server_error() {
        tracing::error!(target: "subtidal", "{line}");
    } else if status.is_client_error() {
        tracing::warn!(target: "subtidal", "{line}");
    } else {
        tracing::debug!(target: "subtidal", "{line}");
    }
}
