
// One ISO-BMFF box: its four-character type and its payload.
type Boxed<'a> = (&'a [u8], &'a [u8]);

// Walk the boxes laid out at the start of `data`.
//
// Stops at the first malformed header rather than guessing, so a
// truncated or unexpected segment yields what was readable instead of
// panicking or running away.
fn boxes(data: &[u8]) -> Vec<Boxed<'_>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let size = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let typ = &data[i + 4..i + 8];
        // size 0 means "to the end of the file"; size 1 means the real
        // length is in the 64-bit field that follows the type.
        let (header, total) = match size {
            0 => (8usize, data.len() - i),
            1 => {
                if i + 16 > data.len() {
                    break;
                }
                let large = u64::from_be_bytes(match data[i + 8..i + 16].try_into() {
                    Ok(b) => b,
                    Err(_) => break,
                }) as usize;
                (16usize, large)
            }
            n if n >= 8 => (8usize, n),
            // A size smaller than its own header cannot be advanced past.
            _ => break,
        };
        if total < header || i + total > data.len() {
            break;
        }
        out.push((typ, &data[i + header..i + total]));
        i += total;
    }
    out
}

// The payload of the first child box of the given type.
fn child<'a>(data: &'a [u8], want: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data)
        .into_iter()
        .find(|(typ, _)| *typ == want.as_slice())
        .map(|(_, payload)| payload)
}

// The FLAC metadata blocks carried in the init segment's `dfLa` box.
//
// The path is fixed by the spec: moov → trak → mdia → minf → stbl →
// stsd → the audio sample entry → dfLa.
fn metadata_blocks(init: &[u8]) -> Option<&[u8]> {
    let moov = child(init, b"moov")?;
    let trak = child(moov, b"trak")?;
    let mdia = child(trak, b"mdia")?;
    let minf = child(mdia, b"minf")?;
    let stbl = child(minf, b"stbl")?;
    let stsd = child(stbl, b"stsd")?;
    // stsd is a FullBox with a count before its entries: 4 bytes of
    // version+flags, then a 4-byte entry_count.
    let entries = stsd.get(8..)?;
    let (_typ, entry) = boxes(entries).into_iter().next()?;
    // An AudioSampleEntry has 28 bytes of fixed fields (reserved,
    // data_reference_index, channel count, sample size, sample rate)
    // before any child boxes begin.
    let children = entry.get(28..)?;
    let dfla = boxes(children)
        .into_iter()
        .find(|(typ, _)| *typ == b"dfLa".as_slice())
        .map(|(_, p)| p)?;
    // dfLa is a FullBox: 4 bytes of version+flags, then the FLAC
    // metadata blocks exactly as a .flac file carries them.
    dfla.get(4..)
}

// Mark the final metadata block as last.
//
// A .flac stream ends its metadata with the high bit of the block header
// set. Inside dfLa that flag may be clear, and a decoder that trusts it
// would read the first frame as another metadata block and reject the
// file.
fn mark_last_block(blocks: &mut [u8]) {
    let mut i = 0usize;
    let mut last: Option<usize> = None;
    while i + 4 <= blocks.len() {
        last = Some(i);
        let len = u32::from_be_bytes([0, blocks[i + 1], blocks[i + 2], blocks[i + 3]]) as usize;
        let Some(next) = i.checked_add(4).and_then(|n| n.checked_add(len)) else {
            break;
        };
        if next > blocks.len() {
            break;
        }
        i = next;
    }
    if let Some(p) = last {
        blocks[p] |= 0x80;
    }
}

// The header of a native FLAC stream: the magic plus the metadata blocks
// lifted out of the MP4 init segment. Returns None when the init segment
// is not MP4-wrapped FLAC, which the caller treats as "cannot rewrap".
pub fn header(init: &[u8]) -> Option<Vec<u8>> {
    let blocks = metadata_blocks(init)?;
    // STREAMINFO is 34 bytes plus its 4-byte header; anything shorter is
    // not a usable stream.
    if blocks.len() < 38 {
        return None;
    }
    let mut out = Vec::with_capacity(4 + blocks.len());
    out.extend_from_slice(b"fLaC");
    out.extend_from_slice(blocks);
    mark_last_block(&mut out[4..]);
    Some(out)
}

// The FLAC frames inside one media segment.
//
// A fragment is `moof` (timing and offsets, all of which the native
// stream re-derives for itself) followed by `mdat`, whose payload is
// already a run of complete FLAC frames. Only the payload survives.
pub fn frames(segment: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (typ, payload) in boxes(segment) {
        if typ == b"mdat" {
            out.extend_from_slice(payload);
        }
    }
    out
}

// The length of a segment's FLAC frames, read from box headers alone.
//
// This is what makes an exact Content-Length affordable. The rewrapped
// length is the header plus every segment's `mdat` payload, and those
// sizes live in the box headers at the very start of each segment — so
// a one-kilobyte prefix per segment answers it, instead of downloading
// the audio to measure it.
//
// None when the prefix does not reach the mdat header, which the caller
// treats as "cannot size this cheaply" rather than guessing.
pub fn mdat_len(prefix: &[u8]) -> Option<u64> {
    let mut i = 0usize;
    while i + 8 <= prefix.len() {
        let size = u32::from_be_bytes([prefix[i], prefix[i + 1], prefix[i + 2], prefix[i + 3]]);
        let typ = &prefix[i + 4..i + 8];
        let (header, total) = match size {
            // "to end of file" gives no length to skip by, and a size
            // below its own header cannot be advanced past.
            0 => return None,
            1 => {
                let bytes = prefix.get(i + 8..i + 16)?;
                (16u64, u64::from_be_bytes(bytes.try_into().ok()?))
            }
            n if n >= 8 => (8u64, n as u64),
            _ => return None,
        };
        if total < header {
            return None;
        }
        if typ == b"mdat" {
            return Some(total - header);
        }
        i = i.checked_add(total as usize)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build one box: 4-byte big-endian size, 4-byte type, payload.
    fn mp4_box(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(typ);
        out.extend_from_slice(payload);
        out
    }

    // A metadata block: 1 byte flags+type, 3 bytes length, then data.
    fn meta_block(block_type: u8, last: bool, data: &[u8]) -> Vec<u8> {
        let mut out = vec![block_type | if last { 0x80 } else { 0 }];
        out.extend_from_slice(&(data.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(data);
        out
    }

    fn streaminfo() -> Vec<u8> {
        // Contents are irrelevant here; only the 34-byte length matters.
        meta_block(0, false, &[0xAB; 34])
    }

    // An init segment shaped like Tidal's: the fixed box path down to a
    // dfLa carrying STREAMINFO.
    fn init_segment(blocks: &[u8]) -> Vec<u8> {
        let mut dfla = vec![0u8; 4]; // version + flags
        dfla.extend_from_slice(blocks);
        let mut entry = vec![0u8; 28]; // AudioSampleEntry fixed fields
        entry.extend_from_slice(&mp4_box(b"dfLa", &dfla));
        let sample_entry = mp4_box(b"fLaC", &entry);
        let mut stsd = vec![0u8; 8]; // version+flags, entry_count
        stsd.extend_from_slice(&sample_entry);
        let stbl = mp4_box(b"stsd", &stsd);
        let minf = mp4_box(b"stbl", &stbl);
        let mdia = mp4_box(b"minf", &minf);
        let trak = mp4_box(b"mdia", &mdia);
        let moov = mp4_box(b"trak", &trak);
        let mut out = mp4_box(b"ftyp", b"isom");
        out.extend_from_slice(&mp4_box(b"moov", &moov));
        out
    }

    #[test]
    fn header_is_the_magic_plus_the_metadata_from_dfla() {
        let h = header(&init_segment(&streaminfo())).expect("parses");
        assert_eq!(&h[..4], b"fLaC");
        // 4 magic + 4 block header + 34 STREAMINFO.
        assert_eq!(h.len(), 42);
        // Block type 0 is STREAMINFO, and it must be flagged last or a
        // decoder reads the first audio frame as more metadata.
        assert_eq!(h[4] & 0x7f, 0);
        assert_eq!(h[4] & 0x80, 0x80, "last-block flag must be set");
    }

    #[test]
    fn the_last_block_is_flagged_even_with_several_blocks() {
        // STREAMINFO followed by a padding block, neither flagged.
        let mut blocks = streaminfo();
        blocks.extend_from_slice(&meta_block(1, false, &[0u8; 16]));
        let h = header(&init_segment(&blocks)).expect("parses");
        // The first block keeps its flag clear...
        assert_eq!(h[4] & 0x80, 0);
        // ...and only the trailing one is marked.
        let padding_header = 4 + 4 + 34;
        assert_eq!(h[padding_header] & 0x7f, 1);
        assert_eq!(h[padding_header] & 0x80, 0x80);
    }

    #[test]
    fn an_already_flagged_block_stays_flagged() {
        let h = header(&init_segment(&meta_block(0, true, &[0xAB; 34]))).expect("parses");
        assert_eq!(h[4] & 0x80, 0x80);
    }

    #[test]
    fn a_non_flac_init_segment_is_refused() {
        // No moov at all.
        assert!(header(&mp4_box(b"ftyp", b"isom")).is_none());
        // A moov whose path does not reach a dfLa.
        let moov = mp4_box(b"moov", &mp4_box(b"mvhd", &[0u8; 100]));
        assert!(header(&moov).is_none());
        // A dfLa too short to hold STREAMINFO.
        assert!(header(&init_segment(&meta_block(0, false, &[0u8; 4]))).is_none());
    }

    #[test]
    fn frames_keeps_only_the_mdat_payload() {
        let mut seg = mp4_box(b"moof", &[0x11; 40]);
        seg.extend_from_slice(&mp4_box(b"mdat", &[0x22; 100]));
        let f = frames(&seg);
        assert_eq!(f.len(), 100);
        assert!(f.iter().all(|b| *b == 0x22), "moof must not leak into the audio");
    }

    #[test]
    fn frames_concatenates_several_mdats_in_order() {
        let mut seg = mp4_box(b"styp", b"msdh");
        seg.extend_from_slice(&mp4_box(b"moof", &[0x11; 8]));
        seg.extend_from_slice(&mp4_box(b"mdat", &[0xAA; 10]));
        seg.extend_from_slice(&mp4_box(b"moof", &[0x11; 8]));
        seg.extend_from_slice(&mp4_box(b"mdat", &[0xBB; 20]));
        let f = frames(&seg);
        assert_eq!(f.len(), 30);
        assert_eq!(f[0], 0xAA);
        assert_eq!(f[29], 0xBB);
    }

    #[test]
    fn a_truncated_box_stops_the_walk_instead_of_panicking() {
        // A header claiming more bytes than exist.
        let mut seg = 999u32.to_be_bytes().to_vec();
        seg.extend_from_slice(b"mdat");
        seg.extend_from_slice(&[0u8; 10]);
        assert!(frames(&seg).is_empty());
        // A size smaller than the header itself must not loop forever.
        let mut seg = 2u32.to_be_bytes().to_vec();
        seg.extend_from_slice(b"mdat");
        assert!(frames(&seg).is_empty());
        // Garbage is simply not FLAC.
        assert!(header(&[0u8; 64]).is_none());
        assert!(frames(&[]).is_empty());
    }

    #[test]
    fn a_64_bit_box_size_is_understood() {
        // size=1 means the real length follows the type as a u64. Large
        // mdat boxes use this form.
        let payload = [0x33u8; 50];
        let mut seg = 1u32.to_be_bytes().to_vec();
        seg.extend_from_slice(b"mdat");
        seg.extend_from_slice(&(16u64 + payload.len() as u64).to_be_bytes());
        seg.extend_from_slice(&payload);
        assert_eq!(frames(&seg).len(), 50);
    }
}

#[cfg(test)]
mod sizing_tests {
    use super::*;

    fn mp4_box(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(typ);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn mdat_len_is_read_from_headers_without_the_audio() {
        let mut seg = mp4_box(b"moof", &[0x11; 200]);
        seg.extend_from_slice(&mp4_box(b"mdat", &[0x22; 500_000]));
        // Only the first kilobyte is needed to learn the payload size,
        // which is the whole point: no audio is downloaded to measure.
        let prefix = &seg[..1024];
        assert_eq!(mdat_len(prefix), Some(500_000));
        // And it agrees with what the full extraction produces.
        assert_eq!(frames(&seg).len() as u64, mdat_len(prefix).unwrap());
    }

    #[test]
    fn a_leading_styp_is_skipped() {
        let mut seg = mp4_box(b"styp", b"msdh");
        seg.extend_from_slice(&mp4_box(b"moof", &[0x11; 100]));
        seg.extend_from_slice(&mp4_box(b"mdat", &[0x22; 4096]));
        assert_eq!(mdat_len(&seg[..512]), Some(4096));
    }

    #[test]
    fn a_64_bit_mdat_reports_its_real_length() {
        let mut seg = mp4_box(b"moof", &[0x11; 16]);
        seg.extend_from_slice(&1u32.to_be_bytes());
        seg.extend_from_slice(b"mdat");
        seg.extend_from_slice(&(16u64 + 9000).to_be_bytes());
        assert_eq!(mdat_len(&seg), Some(9000));
    }

    #[test]
    fn a_prefix_too_short_to_reach_mdat_reports_nothing() {
        // A moof larger than the prefix: better to say "unknown" than to
        // guess a length the body will not match.
        let mut seg = mp4_box(b"moof", &[0x11; 4000]);
        seg.extend_from_slice(&mp4_box(b"mdat", &[0x22; 100]));
        assert_eq!(mdat_len(&seg[..1024]), None);
        assert_eq!(mdat_len(&[]), None);
        // A zero size ("to end of file") gives nothing to skip by.
        let mut bad = 0u32.to_be_bytes().to_vec();
        bad.extend_from_slice(b"moof");
        assert_eq!(mdat_len(&bad), None);
    }
}
