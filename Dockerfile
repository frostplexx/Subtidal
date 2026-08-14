# Build stage: full Rust toolchain plus the C toolchain that
# aws-lc-sys needs (clang, cmake, perl for ring, nasm for the
# x86_64 assembly).
FROM rust:1.97-bookworm AS builder
WORKDIR /build

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      clang cmake nasm perl make \
 && rm -rf /var/lib/apt/lists/*

# Compile the dependency tree once. The app crate is rebuilt after the
# real source is copied in below.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
 && cargo build --release --locked \
 && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release --locked

# Runtime stage: glibc + CA certificates, non-root user, writable /data
# for the Tidal token file.
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --create-home subtidal \
 && mkdir -p /data \
 && chown subtidal:subtidal /data

COPY --from=builder /build/target/release/subtidal /usr/local/bin/subtidal

USER subtidal
ENV SUBTIDAL_TOKEN_FILE=/data/tokens.json
EXPOSE 8000
# /rest/ping is the public Subsonic ping endpoint.
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8000/rest/ping > /dev/null || exit 1
ENTRYPOINT ["subtidal"]
