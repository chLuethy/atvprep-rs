#!/usr/bin/env bash
# Build/run in an isolated prefix; no production Docker services are touched.
set -euo pipefail
image=${1:?usage: container_smoke.sh IMAGE ARTIFACT_DIRECTORY}
artifacts=${2:?usage: container_smoke.sh IMAGE ARTIFACT_DIRECTORY}
repo=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$artifacts/config" "$artifacts/state"
artifacts=$(cd "$artifacts" && pwd)
case "$artifacts/" in "$repo/"*) echo 'Artifacts must be outside the worktree' >&2; exit 2;; esac
umask 077
openssl rand -hex 32 > "$artifacts/config/master-token"
cp "$repo/rust/release/config.toml" "$artifacts/config/config.toml"
container=$(docker run -d --user "$(id -u):$(id -g)" \
  -v "$artifacts/config:/config:ro" -v "$artifacts/state:/state" \
  -p 127.0.0.1::8765 "$image")
cleanup() {
  docker logs "$container" > "$artifacts/container.log" 2>&1 || true
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$artifacts/config/master-token"
}
trap cleanup EXIT
for attempt in $(seq 1 60); do
  status=$(docker inspect --format '{{.State.Health.Status}}' "$container")
  if [ "$status" = healthy ]; then break; fi
  if [ "$status" = unhealthy ]; then echo 'Container is unhealthy' >&2; exit 1; fi
  sleep 1
done
[ "$status" = healthy ]
docker exec "$container" sh -ec \
  'atvprep version; atvprep-manager version; ! command -v python3; ! command -v uv; test -x /usr/bin/ffmpeg; test -x /usr/bin/ffprobe; test -x /usr/bin/mp4info; test -x /usr/bin/mp4dump; wget -qO- http://127.0.0.1:8765/api/health/ready' \
  > "$artifacts/smoke.log"
docker inspect --format '{{.Image}}' "$container" > "$artifacts/image-id"
printf 'Container smoke passed; artifacts: %s\n' "$artifacts"
