# syntax=docker/dockerfile:1
FROM node:22.22.3-alpine@sha256:e58326d0d441090181ac150dc2078d3e2cf6a0d42e809aebba3ef5880935ffdd AS web
WORKDIR /web
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --ignore-scripts
COPY frontend ./
RUN npm run build -- --outDir /web/dist

FROM rust:1.99.1-alpine@sha256:7e9e2d068a107a2adafd19930011178d14a78326053cd4c3a9f7ad3fd049557d AS binary
RUN apk add --no-cache build-base perl pkgconf binutils
# The binary embeds the built frontend (crates/atvprep-api/build.rs reads ../../../frontend/dist),
# so the repository layout is kept and the web stage runs first.
WORKDIR /src/rust
COPY rust/Cargo.toml rust/Cargo.lock rust/rust-toolchain.toml ./
COPY rust/crates ./crates
COPY --from=web /web/dist /src/frontend/dist
ENV RUSTFLAGS="--remap-path-prefix=/src=." SOURCE_DATE_EPOCH=0
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/rust/target \
    cargo build --locked --release -p atvprep && cp target/release/atvprep /atvprep \
 && readelf -l /atvprep > /tmp/elf-headers && ! grep -q INTERP /tmp/elf-headers

FROM scratch AS artifacts
COPY --from=binary /atvprep /atvprep

FROM alpine:3.24.3@sha256:be2656061b536c1883c4034790e21d0a09c5aaaf51968b89368a3c0371ea2c9a AS runtime
RUN apk add --no-cache ffmpeg bento4 ca-certificates \
 && mkdir -p /config /state /media \
 && chown 1000:1000 /config /state /media
COPY --from=binary /atvprep /usr/local/bin/atvprep
RUN ln -s atvprep /usr/local/bin/atvprep-hook \
 && ln -s atvprep /usr/local/bin/atvprep-manager \
 && atvprep version && ffprobe -version \
 && test -x /usr/bin/mp4dump && test -x /usr/bin/mp4info \
 && ! command -v python3 && ! command -v uv
COPY --from=web /web/dist /usr/share/atvprep/web_dist
USER 1000:1000
EXPOSE 8765
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
 CMD wget -q -O /dev/null http://127.0.0.1:8765/api/health/ready || exit 1
ENTRYPOINT ["/usr/local/bin/atvprep"]
CMD ["manager","api","--config","/config/config.toml","--web-dist","/usr/share/atvprep/web_dist"]
