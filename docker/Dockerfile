# check=skip=InvalidDefaultArgInFrom
# Ein Image-Rezept für Sonarr, Radarr und Whisparr v3: eigenes FFmpeg (mit Chromaprint), Bento4 und GPAC (MP4Box),
# dazu mediainfo und mkvtoolnix. Die atvprep-Binary ist NICHT im Image: Sie kommt als signierter Hook
# per `atvprep deploy servarr` ins /config-Volume (siehe DEPLOYMENT.md); ein Release braucht so keinen
# Image-Rebuild. Python und uv fügt es nicht hinzu (die hotio-Basis von Whisparr bringt ein eigenes python3 mit).
#
#   . docker/bases.env
#   docker build -f docker/Dockerfile --build-arg BASE_IMAGE="$SONARR_BASE_IMAGE" \
#     --build-arg ALPINE_VERSION="$SONARR_ALPINE_VERSION" -t atvprep-sonarr:<tag> .
#   (Radarr und Whisparr analog mit RADARR_* beziehungsweise WHISPARR_*; docker/build-image.sh baut alle drei.)
#
# BASE_IMAGE und ALPINE_VERSION haben absichtlich keinen Standardwert: Basis (Tag UND Digest) und deren
# Alpine-Version wählt der Betreiber; ein Build ohne sie scheitert. (Die check-Direktive oben unterdrückt
# genau diese Linter-Warnung.)

ARG BASE_IMAGE
ARG ALPINE_VERSION

# ── Stage 1: FFmpeg, Bento4 and GPAC (MP4Box) from source, verified against pinned hashes ───────────────────
FROM alpine:${ALPINE_VERSION} AS builder

ARG FFMPEG_VERSION=9.0.2
ARG FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
ARG BENTO4_VERSION=1.6.0-641
ARG BENTO4_SHA256=e6575877708526e399cff752b4ee23cb1e2ac32fb251e2ca0a1e8d691fd172a7
ARG GPAC_VERSION=26.07.0
ARG GPAC_SHA256=57822c1a74dcb83d76ff1f671e1b4fae2e7614e8194a5adb9f20661e0e9421dd

RUN apk add --no-cache build-base cmake nasm yasm pkgconfig wget coreutils \
    x265-dev dav1d-dev opus-dev libvorbis-dev openssl-dev zlib-dev chromaprint-dev

WORKDIR /tmp/ffmpeg
RUN wget -q -O ffmpeg.tar.xz "https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz" \
 && echo "${FFMPEG_SHA256}  ffmpeg.tar.xz" | sha256sum -c - \
 && tar -xf ffmpeg.tar.xz --strip-components=1

# Codecs atvprep uses: libx265 (HEVC encode), libdav1d (AV1 decode), libopus, libvorbis; the rest is built in.
RUN ./configure \
        --prefix=/usr/local \
        --enable-gpl --enable-version3 --enable-openssl \
        --enable-libx265 --enable-libdav1d --enable-libopus --enable-libvorbis --enable-chromaprint \
        --enable-decoder=aac,ac3,eac3,dts,mlp,truehd,flac,vorbis,opus \
        --enable-decoder=h264,hevc,av1,vp9,vc1 \
        --enable-decoder=subrip,srt,webvtt,ass,mov_text \
        --enable-encoder=libx265,aac,eac3,mov_text \
        --enable-demuxer=matroska,mov,mpegts,avi \
        --enable-muxer=mp4,mov,chromaprint \
        --disable-doc --disable-htmlpages --disable-manpages --disable-podpages --disable-txtpages \
        --disable-ffplay --disable-debug --enable-small \
 && make -j"$(nproc)" \
 && make install DESTDIR=/install \
 && mkdir -p /install/usr/local/lib \
 && for lib in libx265 libdav1d libopus libvorbis libvorbisenc libnuma; do \
      find /usr/lib /lib -name "${lib}.so*" -exec cp -P {} /install/usr/local/lib/ \; 2>/dev/null || true; \
    done

# Bento4 is built from source: the upstream binaries are glibc-linked and do not run on musl.
WORKDIR /tmp/bento4
RUN wget -q -O bento4.tar.gz "https://github.com/axiomatic-systems/Bento4/archive/refs/tags/v${BENTO4_VERSION}.tar.gz" \
 && echo "${BENTO4_SHA256}  bento4.tar.gz" | sha256sum -c - \
 && tar -xzf bento4.tar.gz --strip-components=1 \
 && mkdir cmakebuild && cd cmakebuild \
 && cmake -DCMAKE_BUILD_TYPE=Release .. \
 && make -j"$(nproc)" mp4info mp4dump \
 && mkdir -p /install/usr/local/bin \
 && cp mp4info mp4dump /install/usr/local/bin/

# GPAC: only the MP4Box binary and libgpac, built from source (musl); the optional players, GUI and
# capture modules are switched off. MP4Box rewrites the target file with chunks of about 0.5 s (ADR 0010).
WORKDIR /tmp/gpac
RUN wget -q -O gpac.tar.gz "https://github.com/gpac/gpac/archive/refs/tags/v${GPAC_VERSION}.tar.gz" \
 && echo "${GPAC_SHA256}  gpac.tar.gz" | sha256sum -c - \
 && tar -xzf gpac.tar.gz --strip-components=1 \
 && ./configure --prefix=/usr/local --disable-x11 --use-jack=no --use-oss=no --use-alsa=no --use-pulseaudio=no \
        --use-sdl=no --use-ffmpeg=no --use-libcaca=no \
 && make -j"$(nproc)" \
 && make install DESTDIR=/install

# ── Stage 2: das Servarr-Image ────────────────────────────────────────────────────────────────
FROM ${BASE_IMAGE}

# chromaprint-libs zieht libfftw3 nach; die übrigen Bibliotheken kopiert die Builder-Stage.
RUN apk add --no-cache libssl3 libcrypto3 libgcc libstdc++ zlib mediainfo mkvtoolnix chromaprint-libs

COPY --from=builder /install/usr/local/bin/ /usr/local/bin/
COPY --from=builder /install/usr/local/lib/ /usr/local/lib/

RUN ffmpeg  -version 2>&1 | grep -q "libx265" \
 && ffmpeg  -version 2>&1 | grep -q "enable-chromaprint" \
 && ffmpeg  -hide_banner -muxers 2>&1 | grep -q " chromaprint" \
 && ffprobe -version 2>&1 | grep -q "libx265" \
 && test -x /usr/local/bin/mp4info && test -x /usr/local/bin/mp4dump \
 && MP4Box -version 2>&1 | grep -q "GPAC version" \
 && ! command -v uv
