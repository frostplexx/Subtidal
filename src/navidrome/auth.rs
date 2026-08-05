use md5::{Digest, Md5};

use crate::SETTINGS;
use super::params::QueryParams;

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
            let expected = format!("{:x}", Md5::digest(format!("{}{}", settings.password, salt)));
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
        let expected = format!("{:x}", Md5::digest(password));
        expected.eq_ignore_ascii_case(hex)
    } else {
        p == password
    }
}
