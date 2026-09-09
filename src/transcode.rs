// Transcoding a lossless source into a streaming lossy codec.

use std::io;
use std::sync::LazyLock;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Semaphore;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    Aac,
    Opus,
    Mp3,
}

impl Codec {
    // Names are matched loosely so common spellings of the Subsonic `format`
    // hint all work.
    pub fn from_format(format: Option<&str>) -> Option<Self> {
        match format?.to_ascii_lowercase().as_str() {
            "aac" | "m4a" | "mp4" => Some(Codec::Aac),
            "opus" | "ogg" | "oga" => Some(Codec::Opus),
            "mp3" => Some(Codec::Mp3),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Codec::Aac => "audio/aac",
            Codec::Opus => "audio/ogg",
            Codec::Mp3 => "audio/mpeg",
        }
    }

    pub fn default_bitrate(self) -> u32 {
        match self {
            Codec::Aac => 192,
            Codec::Opus => 128,
            Codec::Mp3 => 192,
        }
    }

    // Fixed per codec so hi-res sources (88.2/96kHz, which MP3 cannot represent
    // at all) still encode. libopus is the strict one: it accepts only
    // 48/24/16/12/8kHz and errors out on anything else, 44100 included.
    fn sample_rate(self) -> u32 {
        match self {
            Codec::Opus => 48_000,
            Codec::Aac | Codec::Mp3 => 44_100,
        }
    }

    fn muxer_args(self, bitrate: u32) -> Vec<String> {
        // The `k` suffix matters: bare `-b:a 128` means 128 *bits* per second,
        // which libopus rejects outright and the other encoders accept only to
        // emit unplayable audio.
        let k = format!("{bitrate}k");
        let rate = self.sample_rate().to_string();
        let mut args: Vec<String> = vec!["-ar".to_string(), rate];
        args.extend(
            match self {
                // ADTS is the streamable form of AAC; a bare .aac file. Raw MP3 and
                // OGG Opus are likewise self-describing streams.
                Codec::Aac => vec!["-c:a", "aac", "-b:a", &k, "-f", "adts"],
                Codec::Opus => vec!["-c:a", "libopus", "-b:a", &k, "-f", "ogg"],
                Codec::Mp3 => vec!["-c:a", "libmp3lame", "-b:a", &k, "-f", "mp3"],
            }
            .into_iter()
            .map(String::from),
        );
        args
    }
}

pub fn target_bitrate(codec: Codec, max_bitrate: Option<u32>) -> u32 {
    match max_bitrate {
        // 0 means "no limit" in Subsonic, so it falls through to the default.
        Some(k) if k > 0 => k.min(320),
        _ => codec.default_bitrate(),
    }
}

fn max_concurrent_transcodes() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8)
        .clamp(8, 32)
}

static TRANSCODE_GATE: LazyLock<Semaphore> =
    LazyLock::new(|| Semaphore::new(max_concurrent_transcodes()));

const STALL_TIMEOUT: Duration = Duration::from_secs(120);

const READ_CHUNK: usize = 64 * 1024;

fn ffmpeg_args(codec: Codec, bitrate: u32) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "flac",
        "-i",
        "pipe:0",
        "-ac",
        "2",
        "-vn",
        "-y",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    args.extend(codec.muxer_args(bitrate));
    args.push("-".to_string());
    args
}

// Whether `bin` can actually be run, checked once at startup.
//
// Transcoding is on by default, so without this a host with no ffmpeg answers
// every lossy-format request with a 200 whose body dies on the first byte —
// worse than not transcoding at all, since those requests otherwise fall back
// to the tier mapping and get Tidal's own lossy asset.
pub async fn ffmpeg_available(bin: &str) -> bool {
    Command::new(bin)
        .args(["-hide_banner", "-loglevel", "error", "-version"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn spawn(bin: &str, codec: Codec, bitrate: u32) -> io::Result<Child> {
    Command::new(bin)
        .args(ffmpeg_args(codec, bitrate))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

// Produce the encoded output for a transcode.
//
// The input is the native FLAC `header` followed by `frames`, which yields each
// segment's FLAC frame bytes in order. Segments are fetched lazily inside
// `frames`, so the client hears the first audio without a full track download.
pub fn transcode<S, E>(
    bin: String,
    codec: Codec,
    bitrate: u32,
    header: Bytes,
    frames: S,
) -> impl Stream<Item = Result<Bytes, io::Error>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(8);

    tokio::spawn(async move {
        // Held for the process lifetime, not just the spawn.
        let _permit = TRANSCODE_GATE.acquire().await.ok();
        let mut child = match spawn(&bin, codec, bitrate).await {
            Ok(child) => child,
            Err(e) => {
                tracing::error!("ffmpeg spawn failed ({bin}): {e}");
                let _ = tx.send(Err(io::Error::other("ffmpeg spawn failed"))).await;
                return;
            }
        };
        let stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");

        // Drain stderr so a full pipe cannot deadlock the child.
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if stderr.read_to_end(&mut buf).await.is_ok() && !buf.is_empty() {
                tracing::debug!("ffmpeg stderr: {}", String::from_utf8_lossy(&buf));
            }
        });

        tokio::spawn(feed(stdin, header, frames));

        match drain(&mut stdout, &tx).await {
            Ok(Outcome::Finished) => {
                let _ = child.wait().await;
            }
            // ffmpeg cannot be waited on here: with stdout no longer drained it
            // blocks on a full pipe forever, and waiting would pin this task,
            // the gate slot, and the process itself.
            Ok(Outcome::Abandoned) => {
                let _ = child.kill().await;
            }
            Err(e) => {
                tracing::debug!("transcode ended: {e}");
                let _ = child.kill().await;
            }
        }
    });

    stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

// Feed the source into ffmpeg's stdin: the native FLAC header, then each
// segment's frames in order. Shutting down `stdin` closes the pipe, which is
// ffmpeg's EOF signal to flush its final frames.
//
// Any error is terminal: the pipe closes early, ffmpeg finishes with what it
// already has, and the client sees a short stream.
async fn feed<S, E>(mut stdin: tokio::process::ChildStdin, header: Bytes, mut frames: S)
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    if !header.is_empty()
        && let Err(e) = stdin.write_all(&header).await
    {
        tracing::debug!("transcode header write failed: {e}");
        return;
    }

    while let Some(chunk) = frames.next().await {
        match chunk {
            Ok(bytes) if bytes.is_empty() => continue,
            Ok(bytes) => {
                // A write error here is the ordinary shape of a client
                // disconnect: ffmpeg was reaped, so its stdin is gone.
                if let Err(e) = stdin.write_all(&bytes).await {
                    tracing::debug!("transcode input write failed: {e}");
                    return;
                }
            }
            Err(e) => {
                tracing::error!("transcode source segment failed: {e}");
                return;
            }
        }
    }

    if let Err(e) = stdin.shutdown().await {
        tracing::debug!("ffmpeg stdin shutdown failed: {e}");
    }
}

enum Outcome {
    Finished,
    Abandoned,
}

async fn drain(
    stdout: &mut tokio::process::ChildStdout,
    tx: &mpsc::Sender<Result<Bytes, io::Error>>,
) -> io::Result<Outcome> {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        let n = stdout.read(&mut buf).await?;
        if n == 0 {
            return Ok(Outcome::Finished);
        }
        let chunk = Bytes::copy_from_slice(&buf[..n]);

        // A send error means the response body was dropped, so the client is
        // gone. A timeout means it is still connected but has not read in a
        // long time, leaving the channel full.
        match tokio::time::timeout(STALL_TIMEOUT, tx.send(Ok(chunk))).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Ok(Outcome::Abandoned),
            Err(_) => {
                tracing::debug!("transcode abandoned: client stalled for {STALL_TIMEOUT:?}");
                return Ok(Outcome::Abandoned);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_names_map_to_codecs() {
        assert_eq!(Codec::from_format(Some("aac")), Some(Codec::Aac));
        assert_eq!(Codec::from_format(Some("M4A")), Some(Codec::Aac));
        assert_eq!(Codec::from_format(Some("opus")), Some(Codec::Opus));
        assert_eq!(Codec::from_format(Some("ogg")), Some(Codec::Opus));
        assert_eq!(Codec::from_format(Some("mp3")), Some(Codec::Mp3));
        assert_eq!(Codec::from_format(Some("flac")), None);
        assert_eq!(Codec::from_format(None), None);
    }

    #[test]
    fn target_bitrate_honors_client_caps() {
        assert_eq!(target_bitrate(Codec::Opus, Some(96)), 96);
        assert_eq!(target_bitrate(Codec::Aac, Some(320)), 320);
        assert_eq!(target_bitrate(Codec::Aac, Some(500)), 320);
        // 0 means "no limit" -> default.
        assert_eq!(
            target_bitrate(Codec::Opus, Some(0)),
            Codec::Opus.default_bitrate()
        );
        assert_eq!(
            target_bitrate(Codec::Mp3, None),
            Codec::Mp3.default_bitrate()
        );
    }

    #[test]
    fn ffmpeg_args_route_stdin_to_stdout() {
        for c in [Codec::Aac, Codec::Opus, Codec::Mp3] {
            let args = ffmpeg_args(c, 128);
            let joined = args.join(" ");
            assert!(
                joined.contains("-f flac"),
                "{c:?} must decode flac: {joined}"
            );
            assert!(joined.contains("-i pipe:0"), "{c:?} must read stdin");
            assert!(joined.contains("-ac 2"), "downmix to stereo");
            // kbps, not bps: libopus errors on a raw bit-per-second value.
            assert!(
                joined.contains("-b:a 128k"),
                "{c:?} bitrate needs a k suffix: {joined}"
            );
            assert!(
                joined.contains(&format!("-ar {}", c.sample_rate())),
                "{c:?} must set its own output rate: {joined}"
            );
            assert!(joined.ends_with('-'), "output on stdout");
        }
        // Opus is the codec that 44100 would break.
        assert_eq!(Codec::Opus.sample_rate(), 48_000);
    }

    #[test]
    fn content_types_and_defaults() {
        assert_eq!(Codec::Aac.content_type(), "audio/aac");
        assert_eq!(Codec::Opus.content_type(), "audio/ogg");
        assert_eq!(Codec::Mp3.content_type(), "audio/mpeg");
        assert_eq!(Codec::Aac.default_bitrate(), 192);
        assert_eq!(Codec::Opus.default_bitrate(), 128);
        assert_eq!(Codec::Mp3.default_bitrate(), 192);
    }

    // A one-second sine as native FLAC. Depends on ffmpeg being on PATH,
    // exactly what production transcoding needs too.
    fn encode_flac() -> Vec<u8> {
        let probe = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-versions"])
            .output();
        if probe.is_err() {
            // Skip rather than fail in lean CI.
            return Vec::new();
        }
        let child = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-ac",
                "2",
                "-ar",
                "44100",
                "-c:a",
                "flac",
                "-f",
                "flac",
                "pipe:1",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("ffmpeg spawn");
        let out = child.wait_with_output().expect("ffmpeg wait");
        assert!(out.status.success(), "ffmpeg failed to make flac");
        out.stdout
    }

    fn decodes_as_audio(bytes: &[u8]) -> bool {
        use std::io::Write;
        let mut child = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                "pipe:0",
                "-f",
                "null",
                "-",
                "-y",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("ffmpeg decode spawn");
        child.stdin.as_mut().unwrap().write_all(bytes).unwrap();
        let out = child.wait_with_output().expect("ffmpeg decode wait");
        out.status.success()
    }

    #[test]
    fn transcode_stream_produces_decodable_audio_for_each_codec() {
        use futures_util::StreamExt as _;
        let flac = Bytes::from(encode_flac());
        if flac.is_empty() {
            return; // ffmpeg unavailable in this environment
        }

        for codec in [Codec::Aac, Codec::Opus, Codec::Mp3] {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            // Split the source the way production does: a header written first,
            // then the rest arriving as later chunks. An empty header with one
            // all-in-one chunk would not exercise the frame stream at all.
            let header = flac.slice(..1024);
            let rest = flac.slice(1024..);
            let chunks: Vec<Result<Bytes, io::Error>> = rest
                .chunks(8192)
                .map(|c| Ok(Bytes::copy_from_slice(c)))
                .collect();
            assert!(chunks.len() > 1, "test needs a multi-chunk frame stream");

            let out: Vec<u8> = rt.block_on(async {
                let frames = futures_util::stream::iter(chunks);
                let frames = Box::pin(frames);
                let stream = transcode("ffmpeg".to_string(), codec, 128, header.clone(), frames);
                let mut stream = Box::pin(stream);
                let mut bytes = Vec::new();
                while let Some(chunk) = stream.next().await {
                    bytes.extend_from_slice(&chunk.unwrap_or_default());
                }
                bytes
            });
            assert!(!out.is_empty(), "{codec:?} produced no output");
            assert!(decodes_as_audio(&out), "{codec:?} output failed to decode");
        }
    }

    // A client that walks away mid-transcode (a skip) must not pin its gate
    // slot, or the gate exhausts and later transcodes hang with headers sent
    // but no body.
    #[test]
    fn abandoning_a_transcode_releases_its_gate_slot() {
        use futures_util::StreamExt as _;
        let flac = Bytes::from(encode_flac());
        if flac.is_empty() {
            return; // ffmpeg unavailable in this environment
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let before = TRANSCODE_GATE.available_permits();

            // The source stays open, as a real one does while segments are
            // still being fetched, so ffmpeg never reaches EOF and never exits
            // on its own — the case where waiting on it hangs forever. Dropping
            // the stream is how hyper drops a body when the client disconnects.
            let frames = futures_util::stream::iter(vec![Ok::<_, io::Error>(flac.clone())])
                .chain(futures_util::stream::pending::<Result<Bytes, io::Error>>());
            let stream = transcode(
                "ffmpeg".to_string(),
                Codec::Mp3,
                128,
                Bytes::new(),
                Box::pin(frames),
            );
            let mut stream = Box::pin(stream);
            let _ = stream.next().await;
            drop(stream);

            for _ in 0..100 {
                if TRANSCODE_GATE.available_permits() >= before {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("transcode gate slot was not released after the client left");
        });
    }

    // The startup probe is what keeps a host without ffmpeg on the working
    // fallback path instead of serving 200s whose body dies immediately.
    #[test]
    fn ffmpeg_probe_rejects_a_binary_it_cannot_run() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            assert!(!ffmpeg_available("subtidal-no-such-ffmpeg-binary").await);
            // A real ffmpeg on PATH must pass, or the probe would disable
            // transcoding on every host.
            if !encode_flac().is_empty() {
                assert!(ffmpeg_available("ffmpeg").await);
            }
        });
    }
}
