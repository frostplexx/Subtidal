// The audio quality tier, the single vocabulary shared by the metadata
// mapping (what a track *is*), the stream handler (what to ask Tidal
// for) and the transcode decision (what we tell the client we serve).
//
// Before this module each of those three kept its own tier strings and
// they disagreed: the stream path spelled hi-res "HI_RES", the
// transcode path spelled it "HIRES_LOSSLESS", and the mapping had a
// private enum. Tiers now only exist as this type; the strings appear
// once each, at the edges (Tidal's `audioquality` param, the settings
// file, Subsonic's format hints).
use serde_json::Value;

// Ordered worst to best. The order is the capping rule: a request is
// served at `min(what was asked for, what the track actually is)`, so
// the derive below is load-bearing, not cosmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    Low,
    High,
    Lossless,
    HiRes,
    // Atmos sits above hi-res because Tidal treats it as the premium
    // presentation of a track, but it is not "more lossless": it is a
    // different (lossy, multichannel) codec. It ranks last only so that
    // `min` never silently downgrades an Atmos request on an Atmos
    // track.
    Atmos,
}

impl Quality {
    // The track's own tier, from Tidal's `mediaMetadata.tags` (badge
    // tags: "DOLBY_ATMOS", "HIRES_LOSSLESS", "LOSSLESS") and/or the
    // `audioQuality` field. ATMOS wins over HIRES_LOSSLESS wins over
    // LOSSLESS; HIGH and LOW are the lossy tiers.
    //
    // None means the payload carried no quality metadata at all. That
    // is not the same as "low quality": the v2-flattened jsonapi track
    // objects (the `album_with_items` fallback, some search and mix
    // feeds) carry neither field, so callers must treat None as "no
    // information" and not as a reason to downgrade.
    pub fn from_track(v: &Value) -> Option<Self> {
        let tags: &[Value] = v["mediaMetadata"]["tags"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let has_tag = |t: &str| tags.iter().any(|x| x.as_str() == Some(t));
        let audio_quality = v["audioQuality"].as_str();
        if has_tag("DOLBY_ATMOS") {
            return Some(Quality::Atmos);
        }
        if has_tag("HIRES_LOSSLESS") || audio_quality == Some("HIRES_LOSSLESS") {
            return Some(Quality::HiRes);
        }
        if has_tag("LOSSLESS") || audio_quality == Some("LOSSLESS") {
            return Some(Quality::Lossless);
        }
        match audio_quality {
            Some("HIGH") => Some(Quality::High),
            Some("LOW") => Some(Quality::Low),
            _ => None,
        }
    }

    // The value for the v1 playbackinfo `audioquality` parameter.
    //
    // Atmos maps to LOSSLESS deliberately: `audioquality` has no Atmos
    // member (the SDK's own AudioQuality union is
    // HI_RES_LOSSLESS|LOSSLESS|HIGH|LOW). Atmos is the orthogonal
    // `audioMode` and is selected by the *track*, not by this param —
    // asking for an Atmos track at LOSSLESS is what returns the Atmos
    // asset. The response's `audioMode` is what confirms it.
    pub fn as_audioquality(self) -> &'static str {
        match self {
            Quality::Low => "LOW",
            Quality::High => "HIGH",
            Quality::Lossless | Quality::Atmos => "LOSSLESS",
            Quality::HiRes => "HI_RES_LOSSLESS",
        }
    }

    // The configured default tier (the `tidal_quality` setting). An
    // unrecognized value returns None so the caller can warn rather
    // than silently serving something else.
    pub fn from_setting(s: &str) -> Option<Self> {
        match s {
            "LOW" => Some(Quality::Low),
            "HIGH" => Some(Quality::High),
            "LOSSLESS" => Some(Quality::Lossless),
            "HI_RES_LOSSLESS" | "HIRES_LOSSLESS" | "HI_RES" => Some(Quality::HiRes),
            "ATMOS" | "DOLBY_ATMOS" => Some(Quality::Atmos),
            _ => None,
        }
    }

    // The tier a Subsonic client asked for, from its `maxBitRate` (kbps)
    // and `format` hints. None means it expressed no preference and the
    // configured default applies.
    //
    // A bitrate cap is a hard ceiling and wins over the format hint: a
    // client asking for flac at 128 kbps is asking for something that
    // does not exist, and the cap is the half that is actionable. Note
    // maxBitRate=0 means "no limit" in Subsonic, not "silence".
    pub fn from_subsonic(max_bit_rate: Option<u32>, format: Option<&str>) -> Option<Self> {
        match max_bit_rate {
            Some(m) if (1..=64).contains(&m) => Some(Quality::Low),
            Some(m) if (65..=320).contains(&m) => Some(Quality::High),
            // Above 320 kbps the cap cannot distinguish the lossless
            // tiers, so the format hint decides.
            _ => match format {
                // VeloSonic's "Dolby Atmos" option sends eac3 with an
                // unlimited bitrate; it is the only format hint that
                // names a tier Tidal has.
                Some("eac3") | Some("ec-3") => Some(Quality::Atmos),
                Some("flac") => Some(Quality::Lossless),
                // Any other non-empty format is a codec the client can
                // play, not a tier. It carries no tier information, so
                // it must not override the configured default.
                _ => None,
            },
        }
    }

    // The source container and codec advertised to Subsonic clients.
    pub fn content_type_and_suffix(self) -> (&'static str, &'static str) {
        match self {
            Quality::Atmos => ("audio/eac3", "eac3"),
            Quality::HiRes | Quality::Lossless => ("audio/flac", "flac"),
            Quality::High | Quality::Low => ("audio/mp4", "m4a"),
        }
    }

    // The tier's typical bitrate in kbps. Tier-typical, not per-track
    // truth: the track JSON carries no bitrate.
    pub fn bitrate(self) -> u32 {
        match self {
            Quality::Atmos => 768,
            Quality::HiRes => 3000,
            Quality::Lossless => 1411,
            Quality::High => 320,
            Quality::Low => 96,
        }
    }

    // The tier's typical bit depth, same caveat as `bitrate`.
    pub fn bit_depth(self) -> u32 {
        match self {
            Quality::HiRes => 24,
            Quality::Atmos | Quality::Lossless | Quality::High | Quality::Low => 16,
        }
    }

    // The tier's typical sample rate in Hz, same caveat as `bitrate`.
    pub fn sample_rate(self) -> u32 {
        match self {
            Quality::Atmos => 48_000,
            Quality::HiRes => 96_000,
            Quality::Lossless | Quality::High | Quality::Low => 44_100,
        }
    }

    // The tier's channel count: Atmos carries 6, everything else stereo.
    pub fn channel_count(self) -> u32 {
        match self {
            Quality::Atmos => 6,
            Quality::HiRes | Quality::Lossless | Quality::High | Quality::Low => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn track_tier_prefers_tags_over_audio_quality() {
        let atmos = json!({"audioQuality": "LOSSLESS", "mediaMetadata": {"tags": ["LOSSLESS", "DOLBY_ATMOS"]}});
        assert_eq!(Quality::from_track(&atmos), Some(Quality::Atmos));
        let hires = json!({"mediaMetadata": {"tags": ["HIRES_LOSSLESS"]}});
        assert_eq!(Quality::from_track(&hires), Some(Quality::HiRes));
        // audioQuality alone still resolves hi-res.
        assert_eq!(
            Quality::from_track(&json!({"audioQuality": "HIRES_LOSSLESS"})),
            Some(Quality::HiRes)
        );
        assert_eq!(
            Quality::from_track(&json!({"audioQuality": "HIGH"})),
            Some(Quality::High)
        );
        assert_eq!(
            Quality::from_track(&json!({"audioQuality": "LOW"})),
            Some(Quality::Low)
        );
    }

    #[test]
    fn track_without_quality_metadata_is_unknown() {
        // A v2-flattened jsonapi track carries neither field. That must
        // read as "no information", never as a low tier.
        assert_eq!(Quality::from_track(&json!({"id": 1, "title": "x"})), None);
    }

    #[test]
    fn ordering_is_the_capping_rule() {
        assert!(Quality::Low < Quality::High);
        assert!(Quality::High < Quality::Lossless);
        assert!(Quality::Lossless < Quality::HiRes);
        assert!(Quality::HiRes < Quality::Atmos);
        // A lossless request on a lossy track is served lossy.
        assert_eq!(Quality::Lossless.min(Quality::High), Quality::High);
        // A lossy request on a lossless track stays lossy: the client
        // asked for a cap and the cap is honored.
        assert_eq!(Quality::Low.min(Quality::HiRes), Quality::Low);
        // An Atmos request on an Atmos track is not downgraded.
        assert_eq!(Quality::Atmos.min(Quality::Atmos), Quality::Atmos);
    }

    #[test]
    fn atmos_asks_tidal_for_lossless() {
        // `audioquality` has no Atmos member; the track selects it.
        assert_eq!(Quality::Atmos.as_audioquality(), "LOSSLESS");
        assert_eq!(Quality::HiRes.as_audioquality(), "HI_RES_LOSSLESS");
        assert_eq!(Quality::Lossless.as_audioquality(), "LOSSLESS");
        assert_eq!(Quality::High.as_audioquality(), "HIGH");
        assert_eq!(Quality::Low.as_audioquality(), "LOW");
    }

    #[test]
    fn subsonic_bitrate_caps_win_over_format() {
        assert_eq!(Quality::from_subsonic(Some(64), None), Some(Quality::Low));
        assert_eq!(Quality::from_subsonic(Some(128), None), Some(Quality::High));
        assert_eq!(Quality::from_subsonic(Some(320), None), Some(Quality::High));
        assert_eq!(
            Quality::from_subsonic(Some(128), Some("flac")),
            Some(Quality::High)
        );
        assert_eq!(
            Quality::from_subsonic(Some(64), Some("eac3")),
            Some(Quality::Low)
        );
    }

    #[test]
    fn subsonic_format_hints_name_a_tier_only_for_eac3_and_flac() {
        assert_eq!(Quality::from_subsonic(None, Some("eac3")), Some(Quality::Atmos));
        assert_eq!(
            Quality::from_subsonic(None, Some("flac")),
            Some(Quality::Lossless)
        );
        // 0 means "no limit", not a cap.
        assert_eq!(Quality::from_subsonic(Some(0), None), None);
        // An unrelated codec hint expresses no tier, so the configured
        // default must apply instead of being overridden to lossless.
        assert_eq!(Quality::from_subsonic(None, Some("mp3")), None);
        assert_eq!(Quality::from_subsonic(None, None), None);
    }

    #[test]
    fn setting_accepts_every_documented_spelling() {
        assert_eq!(Quality::from_setting("ATMOS"), Some(Quality::Atmos));
        assert_eq!(
            Quality::from_setting("HI_RES_LOSSLESS"),
            Some(Quality::HiRes)
        );
        // The transcode module's older spelling stays accepted so an
        // existing settings file keeps working.
        assert_eq!(Quality::from_setting("HIRES_LOSSLESS"), Some(Quality::HiRes));
        assert_eq!(Quality::from_setting("LOSSLESS"), Some(Quality::Lossless));
        assert_eq!(Quality::from_setting("nonsense"), None);
    }
}
