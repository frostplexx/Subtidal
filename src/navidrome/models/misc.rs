// Fake-empty features: shares and internet radio. Neither has a Tidal
// backend, so the lists are always empty and the element structs exist
// only to document the response shape — they are never constructed.
use serde::Serialize;

// getShares data: { shares: { share: [ Share ] } }
#[derive(Serialize)]
pub struct SharesResponse {
    pub shares: Shares,
}

#[derive(Serialize)]
pub struct Shares {
    pub share: Vec<Share>,
}

#[derive(Serialize)]
pub struct Share {
    pub id: String,
    pub url: String,
    pub description: String,
    pub username: String,
    pub created: String,
    pub expires: String,
}

// getInternetRadioStations data:
// { internetRadioStations: { internetRadioStation: [ InternetRadioStation ] } }
#[derive(Serialize)]
pub struct InternetRadioStationsResponse {
    #[serde(rename = "internetRadioStations")]
    pub internet_radio_stations: InternetRadioStations,
}

#[derive(Serialize)]
pub struct InternetRadioStations {
    #[serde(rename = "internetRadioStation")]
    pub internet_radio_station: Vec<InternetRadioStation>,
}

#[derive(Serialize)]
pub struct InternetRadioStation {
    pub id: String,
    pub name: String,
    #[serde(rename = "streamUrl")]
    pub stream_url: String,
    #[serde(rename = "homepageUrl", skip_serializing_if = "Option::is_none")]
    pub homepage_url: Option<String>,
}
