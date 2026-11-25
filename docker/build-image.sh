#!/usr/bin/env bash
# Baut die drei Servarr-Images (Sonarr, Radarr, Whisparr) aus docker/bases.env und prüft die Werkzeuge.
# Aufruf: docker/build-image.sh --tag TAG [--platform PLATTFORM]
#   --tag       Tag der Images (Pflicht), z. B. 0.2 ergibt atvprep-sonarr:0.2
#   --platform  Zielplattform, z. B. linux/amd64 (Standard: die des Docker-Hosts)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
tag=
platform=
usage() { sed -n '3,5s/^# \{0,1\}//p' "$0" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case "$1" in
    --tag) [ $# -ge 2 ] || usage; tag=$2; shift 2 ;;
    --platform) [ $# -ge 2 ] || usage; platform=$2; shift 2 ;;
    -h | --help) usage ;;
    *) printf 'Unbekanntes Argument: %s\n' "$1" >&2; usage ;;
  esac
done
[ -n "$tag" ] || { printf 'Fehlt: --tag TAG\n' >&2; usage; }
# Docker-Tags: Buchstaben, Ziffern, _ . -, höchstens 128 Zeichen, nicht mit . oder - beginnend.
if ! printf '%s' "$tag" | grep -Eq '^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$'; then
  printf 'Ungültiger Tag: %s\n' "$tag" >&2
  exit 2
fi
# shellcheck disable=SC1091
. "$here/bases.env"
for app in sonarr radarr whisparr; do
  upper=$(printf '%s' "$app" | tr '[:lower:]' '[:upper:]')
  base_var="${upper}_BASE_IMAGE"; alpine_var="${upper}_ALPINE_VERSION"
  image="atvprep-$app:$tag"
  docker build ${platform:+--platform "$platform"} -f "$here/Dockerfile" \
    --build-arg "BASE_IMAGE=${!base_var}" --build-arg "ALPINE_VERSION=${!alpine_var}" \
    -t "$image" "$repo"
  docker run --rm ${platform:+--platform "$platform"} --entrypoint sh "$image" -ec '
    ffmpeg -version | grep -q "enable-chromaprint"
    ffmpeg -hide_banner -loglevel error -f lavfi -i "sine=frequency=440:duration=5" -f chromaprint -fp_format base64 - | grep -q .
    ffprobe -version | grep -q libx265
    test -x /usr/local/bin/mp4info; test -x /usr/local/bin/mp4dump
    MP4Box -version 2>&1 | grep -q "GPAC version"
    command -v mediainfo mkvmerge >/dev/null
    ! command -v uv'
  printf '%s: ok\n' "$image"
done
