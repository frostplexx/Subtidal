# Forced-alignment sidecar for Subtidal v2 lyrics.
#
# A small FastAPI service wrapping Qwen3ForcedAligner from the qwen-asr
# package. Subtidal posts a track's Tidal CDN audio URL plus the lyric
# lines here. This service downloads and decodes the audio, aligns the
# words, and returns per-line word timestamps with char offsets into
# each line's text.
#
# Managed with uv. Requirements live in pyproject.toml.
#   uv sync                 # create the .venv and install deps
#   uv run uvicorn aligner_service:app --host 0.0.0.0 --port 8765
#
# The Qwen weights download from Hugging Face on first request.
import io

import numpy as np
import requests
import soundfile as sf
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

from qwen_asr import Qwen3ForcedAligner

app = FastAPI()

_model = None
MODEL_NAME = "Qwen/Qwen3-ForcedAligner-0.6B"

# The aligner supports 11 languages. Requests outside this set fall back
# to English rather than failing hard.
SUPPORTED = {
    "chinese", "english", "cantonese", "french", "german", "italian",
    "japanese", "korean", "portuguese", "russian", "spanish",
}

# Present as the official iOS media client; the Tidal CDN edge may treat
# other user agents differently.
IOS_UA = "AppleCoreMedia/1.0.0.24A5408d (iPhone; U; CPU OS 27_0 like Mac OS X; en_us)"


class AlignRequest(BaseModel):
    audio_url: str
    language: str = "English"
    text: list[str]


class Word(BaseModel):
    text: str
    startTime: float
    endTime: float
    charStart: int
    charEnd: int


class Line(BaseModel):
    index: int
    value: str
    words: list[Word]


class AlignResponse(BaseModel):
    lines: list[Line]


def _device() -> str:
    """Pick the fastest available device: CUDA, then Apple MPS, then CPU.
    MPS is the Mac's unified-memory GPU via Metal."""
    import torch
    if torch.cuda.is_available():
        return "cuda:0"
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def get_model():
    global _model
    if _model is None:
        # float16 is the safe dtype on MPS; bf16 can hit slow CPU fallbacks.
        _model = Qwen3ForcedAligner.from_pretrained(
            MODEL_NAME,
            dtype="float16",
            device_map=_device(),
        )
    return _model


def load_audio(url: str) -> tuple:
    """Download and decode an audio URL to a mono float32 numpy array
    at the native sample rate, plus that rate.

    Tries libsndfile first (FLAC/WAV/OGG). Falls back to ffmpeg for
    containers libsndfile cannot read, for example AAC in an MP4.
    """
    resp = requests.get(url, timeout=60, headers={"User-Agent": IOS_UA})
    resp.raise_for_status()
    raw = resp.content
    wav, sr = _decode_bytes(raw)
    return wav, sr


def _decode_bytes(raw: bytes) -> tuple:
    buf = io.BytesIO(raw)
    try:
        wav, sr = sf.read(buf, dtype="float32", always_2d=False)
        return wav, int(sr)
    except Exception:
        pass
    # libsndfile could not read it. Try ffmpeg, which decodes mp4/aac.
    import subprocess
    proc = subprocess.run(
        [
            "ffmpeg", "-i", "pipe:0",
            "-f", "f32le", "-acodec", "pcm_f32le", "-ac", "1",
            "-ar", "16000", "pipe:1",
        ],
        input=raw,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if proc.returncode != 0 or not proc.stdout:
        raise HTTPException(
            status_code=422,
            detail="audio decode failed; neither libsndfile nor ffmpeg could read the stream",
        )
    sr = 16000
    wav = np.frombuffer(proc.stdout, dtype=np.float32).copy()
    return wav, sr


def locate_in_line(line: str, token: str, cursor: int) -> tuple[int, int]:
    """Find token's 0-based inclusive char range in line, searching from
    cursor. Returns (-1, -1) when it cannot be matched. Cleaning removes
    punctuation (keeps L/N chars + apostrophe), so tokens are substrings
    of the original line; a cursor keeps repeated words mapping in order.
    """
    if not token:
        return (-1, -1)
    start = line.find(token, cursor)
    if start >= 0:
        return (start, start + len(token) - 1)
    low_line = line.lower()
    low_token = token.lower()
    start = low_line.find(low_token, cursor)
    if start >= 0:
        return (start, start + len(token) - 1)
    return (-1, -1)


@app.post("/align", response_model=AlignResponse)
async def align(req: AlignRequest):
    lang = req.language.strip().lower()
    if not req.text:
        raise HTTPException(status_code=422, detail="text must not be empty")
    if lang not in SUPPORTED:
        print(f"warning: language '{req.language}' not supported; using English")
        lang = "english"
    model_lang = lang[:1].upper() + lang[1:]

    # The aligner takes one transcript and returns one flat, ordered word
    # list for the whole audio. Newlines delimit lyric lines but collapse
    # during tokenization, so we re-attach each word to its line by greedy
    # substring matching against the original lines.
    joined = "\n".join(req.text)
    wav, sr = load_audio(req.audio_url)

    model = get_model()
    # align returns a list with one ForcedAlignResult per audio sample.
    # We pass one audio + one transcript, so take the first element.
    result = model.align(
        audio=(wav, sr),
        text=joined,
        language=model_lang,
    )[0]

    # result is a single ForcedAlignResult; iterate its word items.
    items = list(result.items)

    lines = []
    word_idx = 0
    for line_idx, line_text in enumerate(req.text):
        words = []
        cursor = 0
        # Consume items that belong to this line. A word belongs here if
        # it can be matched within the line's remaining text; otherwise it
        # likely starts the next line, so stop.
        while word_idx < len(items):
            token = items[word_idx].text
            cs, ce = locate_in_line(line_text, token, cursor)
            if cs < 0:
                break
            words.append(Word(
                text=token,
                startTime=float(items[word_idx].start_time),
                endTime=float(items[word_idx].end_time),
                charStart=cs,
                charEnd=ce,
            ))
            cursor = ce + 1
            word_idx += 1
        if words:
            lines.append(Line(index=line_idx, value=line_text, words=words))

    return AlignResponse(lines=lines)
