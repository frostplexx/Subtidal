use super::auth;
use super::handlers;
use super::log::{logged, named, with_params};
use super::params::QueryParams;
use warp::Filter;
use warp::Reply;

// Identifies which private handler a request matched. The private() chain
// maps every matched path to one of these; dispatch() calls the handler.
#[derive(Clone, Copy)]
enum Which {
    GetUser,
    Search3,
    GetCoverArt,
}

impl Which {
    fn name(self) -> &'static str {
        match self {
            Which::GetUser => "getUser",
            Which::Search3 => "search3",
            Which::GetCoverArt => "getCoverArt",
        }
    }
}

// A function to build our routes
pub fn routes() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    logged(
        // public() is the whitelist: only these endpoints run without
        // authentication. Every other endpoint sits behind require_auth().
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

// Private endpoints, as path matchers. Auth runs before these match, so
// unknown paths with bad credentials get a 40 instead of a 404.
fn private() -> impl Filter<Extract = (Which,), Error = warp::Rejection> + Clone {
    get_user()
        .or(search3())
        .unify()
        .or(get_cover_art())
        .unify()
}

// Matches one private path and reports which handler to call.
fn get_user() -> impl Filter<Extract = (Which,), Error = warp::Rejection> + Clone {
    warp::path!("rest" / "getUser")
        .or(warp::path!("rest" / "getUser.view"))
        .unify()
        .and(warp::get().or(warp::post()).unify())
        .map(|| Which::GetUser)
}

fn search3() -> impl Filter<Extract = (Which,), Error = warp::Rejection> + Clone {
    warp::path!("rest" / "search3")
        .or(warp::path!("rest" / "search3.view"))
        .unify()
        .and(warp::get())
        .map(|| Which::Search3)
}

fn get_cover_art() -> impl Filter<Extract = (Which,), Error = warp::Rejection> + Clone {
    warp::path!("rest" / "getCoverArt")
        .or(warp::path!("rest" / "getCoverArt.view"))
        .unify()
        .and(warp::get())
        .map(|| Which::GetCoverArt)
}

// Calls the matched handler with the authenticated params, then tags the
// reply for the log wrapper.
async fn dispatch(
    q: QueryParams,
    raw: String,
    which: Which,
) -> Result<warp::reply::Response, warp::Rejection> {
    let reply = match which {
        Which::GetUser => handlers::get_user().await?.into_response(),
        Which::Search3 => handlers::search3(q).await?.into_response(),
        Which::GetCoverArt => handlers::get_cover_art(q).await?.into_response(),
    };
    let reply = with_params(&raw)(reply);
    Ok(named(which.name())(reply).into_response())
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

fn get_album_list2() -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {    warp::path!("rest" / "getAlbumList2")
        .or(warp::path!("rest" / "getAlbumList2.view"))
        .unify()
        .and(warp::get().or(warp::post()).unify())
        // .and(warp::query::<QueryParams>())
        .and_then(handlers::get_album_list2)
        .map(named("getAlbumList2"))
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
            "/rest/getUser.view",
            "/rest/search3",
            "/rest/getCoverArt",
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
}
