use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes};
use futures_util::{Stream, StreamExt};
use md5::{Digest, Md5};
use warp::Filter;
use warp::reject::{Reject, Rejection};

use crate::SETTINGS;
use super::params::QueryParams;

// hex string of an MD5 digest. digest 0.11 outputs a hybrid-array value,
// which has no LowerHex impl, so encode the bytes manually. Shared with
// the scrobble module (Last.fm API signatures).
pub(crate) fn md5_hex(data: impl AsRef<[u8]>) -> String {
    let digest = Md5::digest(data);
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

// Constant-time string equality. Equal-length strings are compared
// without early exit, so response timing does not reveal how many
// leading bytes match.
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
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
            ct_eq(&expected, &t.to_ascii_lowercase())
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
        ct_eq(&expected, &hex.to_ascii_lowercase())
    } else {
        ct_eq(p, password)
    }
}

// Rejection produced by require_auth on bad credentials. handlers::recover
// turns it into the Subsonic "Wrong username or password" reply.
#[derive(Debug)]
pub struct Unauthorized;
impl Reject for Unauthorized {}

// Rejection for a request body that could not be streamed to completion.
#[derive(Debug)]
pub struct BodyReadFailed;
impl Reject for BodyReadFailed {}

// Rejection for a request body over the size cap.
#[derive(Debug)]
pub struct BodyTooLarge;
impl Reject for BodyTooLarge {}

// Upper bound for request bodies (formPost params, ClientInfo JSON).
const MAX_BODY_BYTES: u64 = 1024 * 1024;

// Read a body stream into Bytes, capped at `max`. A request that
// declares a larger Content-Length is rejected by the header filter
// before this runs; chunked bodies are capped here while streaming.
async fn read_capped<S, B>(stream: S, max: u64) -> Result<Bytes, Rejection>
where
    S: Stream<Item = Result<B, warp::Error>>,
    B: Buf,
{
    let mut out: Vec<u8> = Vec::new();
    let mut stream = std::pin::pin!(stream);
    while let Some(chunk) = stream.next().await {
        let mut chunk = chunk.map_err(|_| warp::reject::custom(BodyReadFailed))?;
        let n = chunk.remaining();
        if out.len().saturating_add(n) as u64 > max {
            return Err(warp::reject::custom(BodyTooLarge));
        }
        out.extend_from_slice(chunk.chunk());
        chunk.advance(n);
    }
    Ok(Bytes::from(out))
}

fn bounded_body(max: u64) -> impl Filter<Extract = (Bytes,), Error = Rejection> + Clone {
    warp::body::content_length_limit(max)
        .or_else(|r: Rejection| async move {
            // No Content-Length header (GET, chunked): not a rejection.
            // The stream read below enforces the cap anyway.
            if r.find::<warp::reject::LengthRequired>().is_some() {
                Ok(())
            } else {
                Err(r)
            }
        })
        .and(warp::body::stream())
        .and_then(move |stream| read_capped(stream, max))
}

// Exponential backoff on failed logins, per client IP: 2s, 4s, 8s, ...
// capped at 10 minutes. Off unless the rate_limit setting is enabled.
// Behind a reverse proxy every client shares the proxy IP, so one
// attacker can lock out legitimate users; that is why it defaults off.
const LOCKOUT_BASE: Duration = Duration::from_secs(2);
const LOCKOUT_MAX: Duration = Duration::from_secs(600);
const LOCKOUT_FAILURE_CAP: u32 = 12;
// Above this many tracked IPs, expired entries are pruned on the next
// failure, so a flood from rotating IPs cannot grow the map unboundedly.
const MAX_TRACKED_IPS: usize = 10_000;

fn lockout_duration(failures: u32, base: Duration, max: Duration) -> Duration {
    let exp = 2u64.saturating_pow(failures.saturating_sub(1).min(16));
    base.saturating_mul(exp as u32).min(max)
}

struct RateState {
    failures: u32,
    locked_until: Instant,
}

struct AuthRateLimiter {
    base: Duration,
    max: Duration,
    failure_cap: u32,
    states: Mutex<HashMap<Option<IpAddr>, RateState>>,
}

impl AuthRateLimiter {
    fn new() -> Self {
        Self::with_params(LOCKOUT_BASE, LOCKOUT_MAX, LOCKOUT_FAILURE_CAP)
    }

    fn with_params(base: Duration, max: Duration, failure_cap: u32) -> Self {
        Self {
            base,
            max,
            failure_cap,
            states: Mutex::new(HashMap::new()),
        }
    }

    // True when the request may proceed. A request inside a lockout
    // window is denied regardless of its credentials. An expired
    // lockout resets the failure count.
    fn check(&self, ip: Option<IpAddr>) -> bool {
        let mut states = self.states.lock().unwrap();
        match states.get_mut(&ip) {
            Some(s) if Instant::now() < s.locked_until => false,
            Some(s) => {
                s.failures = 0;
                true
            }
            None => true,
        }
    }

    fn record_failure(&self, ip: Option<IpAddr>) {
        let mut states = self.states.lock().unwrap();
        if states.len() >= MAX_TRACKED_IPS {
            let now = Instant::now();
            states.retain(|_, s| now < s.locked_until);
        }
        let s = states.entry(ip).or_insert(RateState {
            failures: 0,
            locked_until: Instant::now(),
        });
        s.failures = s.failures.saturating_add(1).min(self.failure_cap);
        s.locked_until = Instant::now() + lockout_duration(s.failures, self.base, self.max);
    }

    fn record_success(&self, ip: Option<IpAddr>) {
        self.states.lock().unwrap().remove(&ip);
    }
}

static RATE_LIMITER: OnceLock<AuthRateLimiter> = OnceLock::new();

fn rate_limit_enabled() -> bool {
    SETTINGS.get().map(|s| s.rate_limit).unwrap_or(false)
}

// Auth middleware: merge the URL query and the form-encoded body into
// QueryParams (clients send params in the URL for GET or in the body for
// POST, the OpenSubsonic formPost extension), authenticate, and reject
// with Unauthorized on bad credentials. With the rate_limit setting on,
// repeated failures from one IP double the lockout before the credential
// check. Yields the merged params, the raw query string, the raw
// body bytes, the x-forwarded-proto header, and the host header for
// handlers that build absolute URLs (the HLS multivariant playlist).
pub fn require_auth() -> impl Filter<Extract = (QueryParams, String, Bytes, Option<String>, Option<String>), Error = Rejection> + Clone {
    warp::query::raw()
        .or_else(|_| async { Ok::<_, Infallible>((String::new(),)) })
        .and(warp::addr::remote())
        .and(warp::header::optional::<String>("x-forwarded-proto"))
        .and(warp::header::optional::<String>("host"))
        .and(bounded_body(MAX_BODY_BYTES))
        .and_then(|query: String, remote: Option<SocketAddr>, proto: Option<String>, host: Option<String>, body: Bytes| async move {
            let merged = QueryParams::merge_raw(&query, &body);
            let params = QueryParams::from_merged(&merged).map_err(|_| warp::reject::reject())?;
            let ip = remote.map(|a| a.ip());
            if rate_limit_enabled() {
                let limiter = RATE_LIMITER.get_or_init(AuthRateLimiter::new);
                if !limiter.check(ip) {
                    return Err(warp::reject::custom(Unauthorized));
                }
                if authenticate(&params) {
                    limiter.record_success(ip);
                    Ok((params, merged, body, proto, host))
                } else {
                    limiter.record_failure(ip);
                    Err(warp::reject::custom(Unauthorized))
                }
            } else if authenticate(&params) {
                Ok((params, merged, body, proto, host))
            } else {
                Err(warp::reject::custom(Unauthorized))
            }
        })
        .untuple_one()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_and_mismatches() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(ct_eq("", ""));
        assert!(!ct_eq("", "a"));
        // Case matters inside ct_eq; the callers lowercase client hex
        // input before comparing.
        assert!(!ct_eq("abc", "ABC"));
        assert!(ct_eq(&md5_hex("x"), &md5_hex("x")));
    }

    #[test]
    fn lockout_delay_doubles_and_caps() {
        let base = Duration::from_secs(2);
        let max = Duration::from_secs(600);
        assert_eq!(lockout_duration(1, base, max), Duration::from_secs(2));
        assert_eq!(lockout_duration(2, base, max), Duration::from_secs(4));
        assert_eq!(lockout_duration(3, base, max), Duration::from_secs(8));
        assert_eq!(lockout_duration(4, base, max), Duration::from_secs(16));
        assert_eq!(lockout_duration(9, base, max), Duration::from_secs(512));
        assert_eq!(lockout_duration(10, base, max), Duration::from_secs(600));
        assert_eq!(lockout_duration(99, base, max), Duration::from_secs(600));
    }

    #[test]
    fn failure_locks_then_expires() {
        let limiter = AuthRateLimiter::with_params(
            Duration::from_millis(5),
            Duration::from_millis(100),
            12,
        );
        let ip = None;
        assert!(limiter.check(ip));
        limiter.record_failure(ip);
        assert!(!limiter.check(ip), "a failed attempt must lock the IP");
        std::thread::sleep(Duration::from_millis(30));
        assert!(limiter.check(ip), "the lockout must expire");
    }

    #[test]
    fn success_clears_the_lockout() {
        let limiter = AuthRateLimiter::with_params(
            Duration::from_secs(60),
            Duration::from_secs(600),
            12,
        );
        let ip = None;
        limiter.record_failure(ip);
        assert!(!limiter.check(ip));
        limiter.record_success(ip);
        assert!(limiter.check(ip));
    }

    #[test]
    fn expired_lockout_resets_the_counter() {
        let limiter = AuthRateLimiter::with_params(
            Duration::from_millis(1),
            Duration::from_secs(600),
            12,
        );
        let ip = Some("203.0.113.1".parse().unwrap());
        limiter.record_failure(ip);
        assert!(!limiter.check(ip));
        std::thread::sleep(Duration::from_millis(10));
        assert!(limiter.check(ip), "the lockout must expire");
        // The counter reset on expiry, so the next failure starts again
        // at the base delay instead of doubling from the old count.
        limiter.record_failure(ip);
        assert!(!limiter.check(ip));
    }
}
