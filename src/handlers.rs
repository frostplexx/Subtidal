use super::models::{PingResponse, Post, SubsonicResponse};

// A function to handle GET requests at /posts/{id}
pub async fn get_post(id: u64) -> Result<impl warp::Reply, warp::Rejection> {
    // For simplicity, let's say we are returning a static post
    let post = Post {
        id,
        title: String::from("Hello, Warp!"),
        body: String::from("This is a post about Warp."),
    };
    Ok(warp::reply::json(&post))
}

// A function to handle the OpenSubsonic ping endpoint
pub async fn ping() -> Result<impl warp::Reply, warp::Rejection> {
    Ok(warp::reply::json(&SubsonicResponse {
        status: "ok",
        version: "1.16.1",
        server_type: "HighTide",
        server_version: "0.1.0",
        open_subsonic: true,
        data: PingResponse {},
    }))
}
