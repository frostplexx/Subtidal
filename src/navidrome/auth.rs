use md5::{Digest, Md5};

use super::params::QueryParams;

// Temporary hard-coded credentials. Replace with env config later.
pub const USERNAME: &str = "admin";
pub const PASSWORD: &str = "admin";

// Token authentication per the Subsonic spec:
// token = hex(md5(password + salt)), sent as t with the salt as s.
// Falls back to the p parameter: plaintext, or hex(md5(password))
// prefixed with "enc:" for API version 1.13.0+.
pub fn authenticate(q: &QueryParams) -> bool {
    let Some(u) = &q.u else {
        return false;
    };
    if u != USERNAME {
        return false;
    }
    match (&q.t, &q.s) {
        (Some(t), Some(s)) => {
            let expected = format!("{:x}", Md5::digest(format!("{}{}", PASSWORD, s)));
            expected.eq_ignore_ascii_case(t)
        }
        _ => match &q.p {
            Some(p) => check_password(p),
            None => false,
        },
    }
}

fn check_password(p: &str) -> bool {
    if let Some(hex) = p.strip_prefix("enc:") {
        let expected = format!("{:x}", Md5::digest(PASSWORD));
        expected.eq_ignore_ascii_case(hex)
    } else {
        p == PASSWORD
    }
}
