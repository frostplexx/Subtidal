use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Instant;

use warp::filters::path::FullPath;
use warp::filters::query;
use warp::http::{Method, StatusCode};
use warp::reply::{Reply, WithHeader};
use warp::Filter;

const ROUTE_HEADER: &str = "x-subtidal-route";
const PARAMS_HEADER: &str = "x-subtidal-params";

// Returns a closure that tags a reply with the route's endpoint name.
// Apply it with `.map(named("search3"))` inside each route function, or with
// a runtime name from dispatch. The wrapper in `logged` reads and strips this
// header, so clients never see it.
pub fn named<R: Reply>(name: &str) -> impl Fn(R) -> WithHeader<R> + Clone {
    move |reply| warp::reply::with_header(reply, ROUTE_HEADER, name.to_string())
}

// Tag a reply with the request params (query + form body, redacted) so the
// log wrapper can show them. Same header mechanism as `named`.
pub fn with_params<R: Reply>(params: &str) -> impl Fn(R) -> WithHeader<R> + Clone {
    let redacted = redact_query(params);
    move |reply| warp::reply::with_header(reply, PARAMS_HEADER, redacted.clone())
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
        .and(query::raw().or_else(|_| async { Ok::<_, Infallible>((String::new(),)) }))
        .and(filter)
        .map(
            |start: Instant,
             full: FullPath,
             method: Method,
             remote: Option<SocketAddr>,
             query: String,
             reply: R| {
                let mut resp = reply.into_response();
                let endpoint = resp
                    .headers_mut()
                    .remove(ROUTE_HEADER)
                    .and_then(|v| v.to_str().ok().map(str::to_owned));
                // Prefer the params tagged by routes that read the request
                // body (POST); fall back to the URL query.
                let params = resp
                    .headers_mut()
                    .remove(PARAMS_HEADER)
                    .and_then(|v| v.to_str().ok().map(str::to_owned))
                    .unwrap_or_else(|| redact_query(&query));
                log_request(
                    start,
                    &method,
                    full.as_str(),
                    &params,
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
    query: &str,
    remote: Option<SocketAddr>,
    status: StatusCode,
    endpoint: Option<&str>,
) {
    let endpoint = endpoint.unwrap_or("-");
    let remote = remote.map(|a| a.to_string()).unwrap_or_else(|| "-".into());
    let elapsed = start.elapsed();
    let line = format!(
        "{remote} {endpoint} \"{method} {path}{query}\" {} in {elapsed:?}",
        status.as_u16()
    );
    if status.is_server_error() {
        tracing::error!(target: "subtidal", "{line}");
    } else if status.is_client_error() {
        tracing::warn!(target: "subtidal", "{line}");
    } else {
        tracing::debug!(target: "subtidal", "{line}");
    }
}

// Query params that must never appear in logs. `t` is the auth token and `p`
// the password; both are credential material. Return the query prefixed with
// "?" or an empty string when there is nothing to log.
fn redact_query(query: &str) -> String {
    let redacted = query
        .split('&')
        .map(|pair| {
            let (key, _) = pair.split_once('=').unwrap_or((pair, ""));
            if matches!(key, "t" | "p" | "token" | "password") {
                format!("{key}=***")
            } else {
                pair.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    if redacted.is_empty() {
        redacted
    } else {
        format!("?{redacted}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_query_keeps_other_params() {
        assert_eq!(
            redact_query("u=alice&v=1.16.1&c=symfonium&f=json"),
            "?u=alice&v=1.16.1&c=symfonium&f=json"
        );
    }

    #[test]
    fn redact_query_masks_credentials() {
        assert_eq!(
            redact_query("u=alice&t=abc123&v=1.16.1&p=enc:x"),
            "?u=alice&t=***&v=1.16.1&p=***"
        );
    }

    #[test]
    fn redact_query_empty_stays_empty() {
        assert_eq!(redact_query(""), "");
    }
}
