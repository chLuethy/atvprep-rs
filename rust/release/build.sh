#!/usr/bin/env bash
# Build isolated musl artifacts for both supported Linux architectures.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:?usage: build.sh ABSOLUTE_OUTPUT_DIRECTORY}
mkdir -p "$out"
out=$(cd "$out" && pwd)
case "$out/" in "$repo/"*) echo 'Release artifacts must be outside the worktree' >&2; exit 2;; esac
for architecture in amd64 arm64; do
  case "$architecture" in amd64) platform=linux-x86_64;; arm64) platform=linux-aarch64;; esac
  docker buildx build --platform "linux/$architecture" \
    --file "$repo/rust/release/Dockerfile" --target artifacts \
    --output "type=local,dest=$out/$platform" "$repo"
done
printf 'Unsigned binaries: %s\n' "$out"
