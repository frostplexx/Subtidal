// getTranscodeDecision response models (OpenSubsonic #168). The server
// never transcodes, so canTranscode is always false and transcodeStream
// is never present; sourceStream describes the Tidal stream the client
// will receive.
use serde::Serialize;

// getTranscodeDecision data: { transcodeDecision: TranscodeDecision }
#[derive(Serialize)]
pub struct TranscodeDecisionResponse {
    #[serde(rename = "transcodeDecision")]
    pub transcode_decision: TranscodeDecision,
}

#[derive(Serialize)]
pub struct TranscodeDecision {
    #[serde(rename = "canDirectPlay")]
    pub can_direct_play: bool,
    #[serde(rename = "canTranscode")]
    pub can_transcode: bool,
    #[serde(rename = "transcodeReason")]
    pub transcode_reason: Vec<String>,
    #[serde(rename = "errorReason")]
    pub error_reason: String,
    #[serde(rename = "transcodeParams")]
    pub transcode_params: String,
    #[serde(rename = "sourceStream")]
    pub source_stream: StreamDetails,
    #[serde(rename = "transcodeStream", skip_serializing_if = "Option::is_none")]
    pub transcode_stream: Option<StreamDetails>,
}

// One side of the decision: the source Tidal stream, or (never here) a
// transcoded target.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamDetails {
    pub protocol: String,
    pub container: String,
    pub codec: String,
    pub audio_channels: u32,
    pub audio_bitrate: u32,
    pub audio_profile: String,
    pub audio_samplerate: u32,
    pub audio_bitdepth: u32,
}
