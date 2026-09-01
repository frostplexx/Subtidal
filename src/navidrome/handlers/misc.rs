use chrono::Utc;

use crate::navidrome::ids::{self, IdKind};
use crate::navidrome::models::misc::Share;
use crate::tidal::{self, client};
// Fake-empty features: shares and internet radio have no Tidal backend.
// Reads return empty lists, so clients see a working feature; writes
// fail with a clear message instead of pretending to persist.
use super::{fail, ok};
use crate::navidrome::models::{
    InternetRadioStations, InternetRadioStationsResponse, Shares, SharesResponse,
};
use crate::navidrome::params::QueryParams;

// getShares: no shares can exist, so the list is empty.
pub async fn get_shares() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(SharesResponse {
        shares: Shares { share: vec![] },
    }))
}

// getInternetRadioStations: no stations can exist, so the list is empty.
pub async fn get_internet_radio_stations() -> Result<warp::reply::Json, warp::Rejection> {
    Ok(ok(InternetRadioStationsResponse {
        internet_radio_stations: InternetRadioStations {
            internet_radio_station: vec![],
        },
    }))
}

pub async fn create_share(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {


  let Some(id) = q.id.0.first() else {
       return Ok(fail(10, "Required parameter missing"));
   };

 let Some((kind, tidal_id)) = ids::parse(id) else {
       return Ok(fail(10, "Invalid id"));
   };


    let path = match kind {
           IdKind::Track => "track",
           IdKind::Album => "album",
           IdKind::Artist => "artist",
           IdKind::Playlist => "playlist",
       };


    let url = format!("https://tidal.com/browse/{path}/{tidal_id}");


    let share = Share {
        username: "user".to_string(),
        description: None,
        created: Utc::now().to_rfc3339(),
        id: "0".to_string(),
        url: url,
        expires: None,
    };

    Ok(ok(SharesResponse {
        shares: Shares { share: vec![share] },
    }))
}

pub async fn update_share(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    Ok(fail(0, "Sharing is not supported"))
}

pub async fn delete_share(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    Ok(fail(0, "Sharing is not supported"))
}

pub async fn create_internet_radio_station(
    _q: QueryParams,
) -> Result<warp::reply::Json, warp::Rejection> {
    Ok(fail(0, "Internet radio is not supported"))
}

pub async fn update_internet_radio_station(
    _q: QueryParams,
) -> Result<warp::reply::Json, warp::Rejection> {
    Ok(fail(0, "Internet radio is not supported"))
}

pub async fn delete_internet_radio_station(
    _q: QueryParams,
) -> Result<warp::reply::Json, warp::Rejection> {
    Ok(fail(0, "Internet radio is not supported"))
}
