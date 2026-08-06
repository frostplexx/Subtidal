// Subsonic ID scheme: prefixed, reversible IDs that encode the Tidal ID.
//   track:    t<id>
//   album:    al<id>
//   artist:   ar<id>
//   playlist: pl<id>
// Clients cache IDs between sessions, so the mapping must be deterministic.
// Parse leniently: a bare number means a raw Tidal ID.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    Track,
    Album,
    Artist,
    Playlist,
}

impl IdKind {
    fn prefix(self) -> &'static str {
        match self {
            IdKind::Track => "t",
            IdKind::Album => "al",
            IdKind::Artist => "ar",
            IdKind::Playlist => "pl",
        }
    }
}

pub fn encode(kind: IdKind, id: u64) -> String {
    format!("{}{}", kind.prefix(), id)
}

pub fn encode_track(id: u64) -> String {
    encode(IdKind::Track, id)
}

pub fn encode_album(id: u64) -> String {
    encode(IdKind::Album, id)
}

pub fn encode_artist(id: u64) -> String {
    encode(IdKind::Artist, id)
}

// Decode an ID of the expected kind. Returns None for wrong prefixes.
pub fn decode(kind: IdKind, s: &str) -> Option<u64> {
    let rest = s.strip_prefix(kind.prefix())?;
    rest.parse().ok()
}

// Decode any prefixed ID, returning the kind too. Used by getCoverArt and
// friends that accept album, artist, or playlist IDs.
pub fn parse(s: &str) -> Option<(IdKind, u64)> {
    for kind in [IdKind::Track, IdKind::Album, IdKind::Artist, IdKind::Playlist] {
        if let Some(id) = decode(kind, s) {
            return Some((kind, id));
        }
    }
    None
}

// stream/download: accept a track ID, or a bare number as a raw Tidal ID.
pub fn parse_track_id(s: &str) -> Option<u64> {
    decode(IdKind::Track, s).or_else(|| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        assert_eq!(encode_track(42), "t42");
        assert_eq!(encode_album(7), "al7");
        assert_eq!(decode(IdKind::Track, "t42"), Some(42));
        assert_eq!(decode(IdKind::Album, "al7"), Some(7));
    }

    #[test]
    fn parse_detects_kind() {
        assert_eq!(parse("ar9"), Some((IdKind::Artist, 9)));
        assert_eq!(parse("pl3"), Some((IdKind::Playlist, 3)));
        assert_eq!(parse("t1"), Some((IdKind::Track, 1)));
        assert_eq!(parse("al2"), Some((IdKind::Album, 2)));
        assert_eq!(parse("xyz"), None);
    }

    #[test]
    fn bare_number_is_track() {
        assert_eq!(parse_track_id("t123"), Some(123));
        assert_eq!(parse_track_id("456"), Some(456));
        assert_eq!(parse_track_id("al2"), None);
    }

    #[test]
    fn wrong_kind_rejected() {
        assert_eq!(decode(IdKind::Track, "al2"), None);
    }
}
