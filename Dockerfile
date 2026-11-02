# ─── build ────────────────────────────────────────────────────────────────
FROM rust:1.85-slim-bookworm AS builder

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential pkg-config libopus-dev \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
# Compiled into the binary via include_str! — must be present at build time.
COPY emojis.toml ./
COPY src ./src
COPY assets ./assets
COPY README.md ./

RUN cargo build --release

# ─── runtime ──────────────────────────────────────────────────────────────
FROM python:3.12-slim-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ffmpeg libopus0 ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && pip install --no-cache-dir -U yt-dlp \
 && rm -rf /root/.cache

WORKDIR /app
COPY --from=builder /build/target/release/nexcord /usr/local/bin/nexcord

ENV RUST_LOG="nexcord=info,songbird=warn,serenity=warn" \
    YT_DLP="yt-dlp"

# Assets are compiled into the binary with include_bytes!, so no files are
# needed at runtime. Nothing is written to disk.
USER 65532:65532
CMD ["nexcord"]
