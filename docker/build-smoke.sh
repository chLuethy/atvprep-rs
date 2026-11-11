#!/usr/bin/env bash
# Baut die drei Servarr-Images (Sonarr, Radarr, Whisparr) aus docker/bases.env und prüft die Werkzeuge.
# Optionaler Parameter: Plattform (Standard: die des Docker-Hosts), z. B. linux/amd64.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
platform=${1:-}
# shellcheck disable=SC1091
. "$here/bases.env"
for app in sonarr radarr whisparr; do
  upper=$(printf '%s' "$app" | tr '[:lower:]' '[:upper:]')
  base_var="${upper}_BASE_IMAGE"; alpine_var="${upper}_ALPINE_VERSION"
  tag="atvprep-$app:smoke"
  docker build ${platform:+--platform "$platform"} -f "$here/Dockerfile" \
    --build-arg "BASE_IMAGE=${!base_var}" --build-arg "ALPINE_VERSION=${!alpine_var}" \
    -t "$tag" "$repo"
  docker run --rm ${platform:+--platform "$platform"} --entrypoint sh "$tag" -ec '
    ffmpeg -version | grep -q "enable-chromaprint"
    ffmpeg -hide_banner -loglevel error -f lavfi -i "sine=frequency=440:duration=5" -f chromaprint -fp_format base64 - | grep -q .
    ffprobe -version | grep -q libx265
    test -x /usr/local/bin/mp4info; test -x /usr/local/bin/mp4dump
    command -v mediainfo mkvmerge >/dev/null
    ! command -v uv'
  printf '%s: ok\n' "$tag"
done
