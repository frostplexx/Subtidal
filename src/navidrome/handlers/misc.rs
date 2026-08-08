// Fake-empty features: shares and internet radio have no Tidal backend.
// Reads return empty lists, so clients see a working feature; writes
// fail with a clear message instead of pretending to persist.
use crate::navidrome::models::{
    InternetRadioStations, InternetRadioStationsResponse, Shares, SharesResponse,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};

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

// The four write endpoints: sharing cannot work without a shareable
// media URL of our own, and radio streams are not played by this server.
pub async fn create_share(_q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    Ok(fail(0, "Sharing is not supported"))
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
