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
    // presentation of a track
    Atmos,
}

impl Quality {
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

    pub fn as_audioquality(self) -> &'static str {
        match self {
            Quality::Low => "LOW",
            Quality::High => "HIGH",
            Quality::Lossless | Quality::Atmos => "LOSSLESS",
            Quality::HiRes => "HI_RES_LOSSLESS",
        }
    }

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

    pub fn from_subsonic(max_bit_rate: Option<u32>, format: Option<&str>) -> Option<Self> {
        match max_bit_rate {
            Some(m) if (1..=64).contains(&m) => Some(Quality::Low),
            Some(m) if (65..=320).contains(&m) => Some(Quality::High),
            // Above 320 kbps the cap cannot distinguish the lossless
            // tiers, so the format hint decides.
            _ => match format {
                Some("eac3") | Some("ec-3") => Some(Quality::Atmos),
                Some("flac") => Some(Quality::Lossless),
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
        // The HIRES_LOSSLESS spelling stays accepted so an existing
        // settings file keeps working.
        assert_eq!(Quality::from_setting("HIRES_LOSSLESS"), Some(Quality::HiRes));
        assert_eq!(Quality::from_setting("LOSSLESS"), Some(Quality::Lossless));
        assert_eq!(Quality::from_setting("nonsense"), None);
    }
}
