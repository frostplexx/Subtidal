# Forced-alignment sidecar for Subtidal v2 lyrics, built with uv.
# Build: docker build -t subtidal-aligner aligner/
# This requires a GPU for practical latency; the container falls back to
# CPU when no GPU is present (slow).
FROM nvidia/cuda:12.4.1-runtime-ubuntu22.04

# The official uv static binary. Runs on the amd64 base image.
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uv/bin/uv
ENV PATH="/uv/bin:$PATH"

RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY pyproject.toml uv.lock .python-version ./
# uv installs the pinned Python (3.12) and the project deps into a venv.
RUN uv python install
RUN uv sync --frozen
COPY aligner_service.py .

EXPOSE 8765
CMD ["uv", "run", "--frozen", "uvicorn", "aligner_service:app", "--host", "0.0.0.0", "--port", "8765"]
