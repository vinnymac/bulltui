# syntax=docker/dockerfile:1
#
# bulltui — a single static-ish CLI binary on a minimal distroless runtime.
# Multi-arch: `docker buildx build --platform linux/amd64,linux/arm64 -t bulltui .`

# ---- build -----------------------------------------------------------------
FROM rust:1.92-slim-bookworm AS builder
WORKDIR /src
COPY . .
# arboard's Linux clipboard backend is pure-Rust (x11rb), and TLS rides on
# rustls with a compiled-in CA bundle (webpki-roots) — no OpenSSL, no system
# certs — so no apt packages are needed. Cache the registry + target dir.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p bulltui --bin bulltui \
    && cp target/release/bulltui /usr/local/bin/bulltui

# ---- runtime ---------------------------------------------------------------
# distroless/cc = glibc + libgcc, no shell, runs as nonroot (uid 65532).
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /usr/local/bin/bulltui /usr/local/bin/bulltui
# crossterm speaks ANSI directly (no terminfo db needed); give it a sane TERM.
ENV TERM=xterm-256color
ENTRYPOINT ["/usr/local/bin/bulltui"]
