use serde::Deserialize;

// Query params shared by /rest/* endpoints. All optional at parse time;
// per-endpoint and auth requirements are enforced in handlers/auth.
#[derive(Deserialize)]
pub struct QueryParams {
    pub username: Option<String>,
    pub u: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub p: Option<String>,
    #[allow(dead_code)]
    pub v: Option<String>,
    #[allow(dead_code)]
    pub c: Option<String>,
}
