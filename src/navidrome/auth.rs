use std::convert::Infallible;

use md5::{Digest, Md5};
use warp::Filter;
use warp::reject::{Reject, Rejection};

use crate::SETTINGS;
use super::params::QueryParams;

// hex string of an MD5 digest. digest 0.11 outputs a hybrid-array value,
// which has no LowerHex impl, so encode the bytes manually.
fn md5_hex(data: impl AsRef<[u8]>) -> String {
    let digest = Md5::digest(data);
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

// Credentials come from settings.toml (username/password), set at startup.

// Token authentication per the Subsonic spec:
// token = hex(md5(password + salt)), sent as t with the salt as s.
// Falls back to the p parameter: plaintext, or hex(md5(password))
// prefixed with "enc:" for API version 1.13.0+.
pub fn authenticate(q: &QueryParams) -> bool {
    let Some(settings) = SETTINGS.get() else {
        return false;
    };
    let Some(u) = &q.u else {
        return false;
    };
    if u != &settings.username {
        return false;
    }
    match (&q.t, &q.s) {
        (Some(t), Some(salt)) => {
            let expected = md5_hex(format!("{}{}", settings.password, salt));
            expected.eq_ignore_ascii_case(t)
        }
        _ => match &q.p {
            Some(p) => check_password(p, &settings.password),
            None => false,
        },
    }
}

fn check_password(p: &str, password: &str) -> bool {
    if let Some(hex) = p.strip_prefix("enc:") {
        let expected = md5_hex(password);
        expected.eq_ignore_ascii_case(hex)
    } else {
        p == password
    }
}

// Rejection produced by require_auth on bad credentials. handlers::recover
// turns it into the Subsonic "Wrong username or password" reply.
#[derive(Debug)]
pub struct Unauthorized;
impl Reject for Unauthorized {}

// Auth middleware: merge the URL query and the form-encoded body into
// QueryParams (clients send params in the URL for GET or in the body for
// POST, the OpenSubsonic formPost extension), authenticate, and reject with
// Unauthorized on bad credentials. Yields the merged params for the handler.
pub fn require_auth() -> impl Filter<Extract = (QueryParams, String), Error = Rejection> + Clone {
    warp::query::raw()
        .or_else(|_| async { Ok::<_, Infallible>((String::new(),)) })
        .and(warp::body::bytes())
        .and_then(|query: String, body: warp::hyper::body::Bytes| async move {
            let merged = QueryParams::merge_raw(&query, &body);
            let params = QueryParams::from_merged(&merged).map_err(|_| warp::reject::reject())?;
            if authenticate(&params) {
                Ok((params, merged))
            } else {
                Err(warp::reject::custom(Unauthorized))
            }
        })
        .untuple_one()
}
