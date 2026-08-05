use super::models::{PingResponse, Post, SubsonicBody, SubsonicResponse};

// A function to handle the OpenSubsonic ping endpoint
pub async fn ping() -> Result<impl warp::Reply, warp::Rejection> {
    Ok(warp::reply::json(&SubsonicResponse {
        inner: SubsonicBody {
            status: "ok",
            version: "1.16.1",
            server_type: "HighTide",
            server_version: "0.1.0",
            open_subsonic: true,
            data: PingResponse {},
        },
    }))
}
