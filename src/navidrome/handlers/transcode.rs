// getTranscodeDecision: the OpenSubsonic transcode decision extension.
// This server never transcodes — the "stream" is a 302 to whatever Tidal
// serves — so the decision is always canDirectPlay, canTranscode false.
// Clients like Feishin then keep using /rest/stream; a truthful
// canDirectPlay=false would point them at getTranscodeStream, which does
// not exist here.
use crate::navidrome::ids;
use crate::navidrome::models::{
    StreamDetails, TranscodeDecision, TranscodeDecisionResponse,
};
use crate::navidrome::params::QueryParams;
use super::{fail, ok};

// The source stream as served for the track's quality tier. The numbers
// are the tier's typical values, not per-track truth (the track JSON has
// no sample rate or bitrate); the container/codec drive client decisions,
// and the decision itself never depends on them.
fn source_stream(quality: Option<&str>) -> StreamDetails {
    match quality {
        Some("ATMOS") => StreamDetails {
            protocol: "http".into(),
            container: "mp4".into(),
            codec: "eac3".into(),
            audio_channels: 6,
            audio_bitrate: 768_000, 
            audio_profile: "JOC".into(),
            audio_samplerate: 48_000,
            audio_bitdepth: 16,
        },
        Some("HIRES_LOSSLESS") => StreamDetails {
            protocol: "http".into(),
            container: "flac".into(),
            codec: "flac".into(),
            audio_channels: 2,
            audio_bitrate: 3_000_000,
            audio_profile: String::new(),
            audio_samplerate: 96_000,
            audio_bitdepth: 24,
        },
        Some("LOSSLESS") | None => StreamDetails {
            protocol: "http".into(),
            container: "flac".into(),
            codec: "flac".into(),
            audio_channels: 2,
            audio_bitrate: 1_411_000,
            audio_profile: String::new(),
            audio_samplerate: 44_100,
            audio_bitdepth: 16,
        },
        Some("HIGH") => StreamDetails {
            protocol: "http".into(),
            container: "mp4".into(),
            codec: "aac".into(),
            audio_channels: 2,
            audio_bitrate: 320_000,
            audio_profile: String::new(),
            audio_samplerate: 44_100,
            audio_bitdepth: 16,
        },
        Some("LOW") => StreamDetails {
            protocol: "http".into(),
            container: "mp4".into(),
            codec: "aac".into(),
            audio_channels: 2,
            audio_bitrate: 96_000,
            audio_profile: String::new(),
            audio_samplerate: 44_100,
            audio_bitdepth: 16,
        },
        // Unknown tiers follow the same lossy fallback as the Child
        // content-type mapping (audio/mp4) until proven otherwise.
        Some(_) => StreamDetails {
            protocol: "http".into(),
            container: "mp4".into(),
            codec: "aac".into(),
            audio_channels: 2,
            audio_bitrate: 320_000,
            audio_profile: String::new(),
            audio_samplerate: 44_100,
            audio_bitdepth: 16,
        },
    }
}

// The request body carries the client's capabilities (ClientInfo JSON).
// The decision ignores them: the server cannot transcode, so the answer
// is direct play or nothing, no matter what the client can decode.
pub async fn get_transcode_decision(
    q: QueryParams,
    _body: bytes::Bytes,
) -> Result<warp::reply::Json, warp::Rejection> {
    let Some(media_id) = q.media_id.as_deref().or_else(|| q.id.0.first().map(String::as_str)) else {
        return Ok(fail(10, "Required parameter missing"));
    };
    if q.media_type.as_deref() != Some("song") {
        return Ok(fail(0, "Podcasts are not supported"));
    }
    let Some(track_id) = ids::parse_track_id(media_id) else {
        return Ok(fail(70, "Song not found"));
    };
    let client = crate::tidal::client();
    let detail = match client.track(track_id).await {
        Ok(v) => v.to_json(),
        Err(e) => {
            tracing::error!("tidal track fetch failed: {e}");
            return Ok(fail(0, "Song unavailable"));
        }
    };
    let decision = TranscodeDecision {
        can_direct_play: true,
        can_transcode: false,
        transcode_reason: vec![],
        error_reason: String::new(),
        transcode_params: String::new(),
        source_stream: source_stream(detail["audioQuality"].as_str()),
        transcode_stream: None,
    };
    Ok(ok(TranscodeDecisionResponse {
        transcode_decision: decision,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_stream_follows_quality() {
        let flac = source_stream(Some("LOSSLESS"));
        assert_eq!(flac.container, "flac");
        assert_eq!(flac.codec, "flac");
        assert_eq!(flac.audio_bitdepth, 16);
        let hires = source_stream(Some("HIRES_LOSSLESS"));
        assert_eq!(hires.audio_samplerate, 96_000);
        assert_eq!(hires.audio_bitdepth, 24);
        let high = source_stream(Some("HIGH"));
        assert_eq!(high.container, "mp4");
        assert_eq!(high.codec, "aac");
        assert_eq!(high.audio_bitrate, 320_000);
        let low = source_stream(Some("LOW"));
        assert_eq!(low.audio_bitrate, 96_000);
        let unknown = source_stream(None);
        assert_eq!(unknown.container, "flac");
    }

    #[test]
    fn decision_serializes_open_subsonic_shape() {
        let decision = TranscodeDecision {
            can_direct_play: true,
            can_transcode: false,
            transcode_reason: vec![],
            error_reason: String::new(),
            transcode_params: String::new(),
            source_stream: source_stream(Some("LOSSLESS")),
            transcode_stream: None,
        };
        let json = serde_json::to_value(TranscodeDecisionResponse {
            transcode_decision: decision,
        })
        .unwrap();
        let td = &json["transcodeDecision"];
        assert_eq!(td["canDirectPlay"], true);
        assert_eq!(td["canTranscode"], false);
        assert_eq!(td["transcodeReason"], serde_json::json!([]));
        assert_eq!(td["sourceStream"]["container"], "flac");
        assert_eq!(td["sourceStream"]["audioChannels"], 2);
        assert!(td.get("transcodeStream").is_none());
    }
}
