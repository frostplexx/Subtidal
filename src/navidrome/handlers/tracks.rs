// Track browsing and streaming: getSong plus the stream endpoint.
use crate::navidrome::ids;
use crate::navidrome::models::GetSongResponse;
use crate::navidrome::params::QueryParams;
use super::{fail, ok};
use crate::tidal::mapping::{song_from_track, year_from};
use warp::Reply;

// getSong: one track's detail. The id may be t<id> or a bare number.
// Tidal track JSON carries no release date (not even on the embedded
// album), so the year is filled from the album detail, mirroring getAlbum.
// The album fetch hits the meta cache, so repeat calls cost nothing.
pub async fn get_song(q: QueryParams) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing"));
    };
    let Some(track_id) = ids::decode(ids::IdKind::Track, id).or_else(|| id.parse().ok()) else {
        return Ok(fail(70, "Song not found"));
    };
    let client = crate::tidal::client();
    let detail = match client.track(track_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("tidal track fetch failed: {e}");
            return Ok(fail(0, "Song unavailable"));
        }
    };
    let mut song = match song_from_track(&detail) {
        Some(s) => s,
        None => return Ok(fail(70, "Song not found")),
    };
    if song.year.is_none()
        && let Some(album_id) = detail["album"]["id"].as_u64()
        && let Ok(album) = client.album(album_id).await
    {
        song.year = year_from(album["releaseDate"].as_str());
    }
    Ok(ok(GetSongResponse { song }))
}

// Map Subsonic maxBitRate (kbps) to a Tidal quality tier. Tidal has no
// real transcoding; the tier selects the stream the account can play.
// Bitrate wins when set: <=64 -> LOW (HE-AAC ~96k), <=320 -> HIGH (AAC
// 320k), anything above -> LOSSLESS. Without a bitrate, a lossy format
// hint caps at HIGH; flac or no format asks for LOSSLESS. Tidal cascades
// downward when a track or the account lacks the tier.
fn tidal_quality(max_bit_rate: Option<u32>, format: Option<&str>) -> &'static str {
    match max_bit_rate {
        // 0 means "no limit" in Subsonic; only a positive bitrate caps.
        Some(m) if (1..=64).contains(&m) => "LOW",
        Some(m) if (65..=320).contains(&m) => "HIGH",
        _ => match format {
            Some("flac") => "LOSSLESS",
            Some(f) if !f.is_empty() => "HIGH",
            _ => "LOSSLESS",
        },
    }
}

// stream: resolve a track to a single-file Tidal CDN URL and 302-redirect
// there. The server never touches the audio bytes. Ids are t<id> or bare
// numbers. Hi-res tracks answer DASH (segmented FLAC, no single URL), so
// the handler falls back to HIGH, which is always one file.
pub async fn stream(q: QueryParams) -> Result<warp::reply::Response, warp::Rejection> {
    let Some(id) = q.id.0.first() else {
        return Ok(fail(10, "Required parameter missing").into_response());
    };
    let Some(track_id) = ids::parse_track_id(id) else {
        return Ok(fail(70, "Song not found").into_response());
    };
    let client = crate::tidal::client();
    let mut tier = tidal_quality(q.max_bit_rate, q.format.as_deref());
    let url = loop {
        match client.stream_info(track_id, tier).await {
            Ok(info) if info.direct_url.is_some() => break info.direct_url.unwrap(),
            Ok(info) => {
                tracing::debug!(
                    "tier {tier} returned {} for track {track_id}; falling back to HIGH",
                    info.mime_type
                );
                if tier == "HIGH" {
                    return Ok(fail(0, "Stream unavailable").into_response());
                }
                tier = "HIGH";
            }
            Err(e) => {
                tracing::error!("tidal stream fetch failed for track {track_id}: {e}");
                return Ok(fail(0, "Stream unavailable").into_response());
            }
        }
    };
    tracing::debug!("stream {track_id} tier={tier}");
    Ok(warp::reply::with_header(
        warp::reply::with_status(warp::reply(), warp::http::StatusCode::FOUND),
        "Location",
        url,
    )
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::tidal_quality;

    #[test]
    fn bitrate_picks_tier() {
        assert_eq!(tidal_quality(None, None), "LOSSLESS");
        assert_eq!(tidal_quality(Some(0), None), "LOSSLESS");
        assert_eq!(tidal_quality(Some(64), None), "LOW");
        assert_eq!(tidal_quality(Some(128), None), "HIGH");
        assert_eq!(tidal_quality(Some(320), None), "HIGH");
        assert_eq!(tidal_quality(Some(999), None), "LOSSLESS");
    }

    #[test]
    fn format_hint_only_matters_without_bitrate() {
        assert_eq!(tidal_quality(None, Some("flac")), "LOSSLESS");
        assert_eq!(tidal_quality(None, Some("mp3")), "HIGH");
        assert_eq!(tidal_quality(Some(128), Some("flac")), "HIGH");
        assert_eq!(tidal_quality(Some(64), Some("mp3")), "LOW");
    }
}
