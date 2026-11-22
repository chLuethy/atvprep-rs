#!/usr/bin/env bash
# Build isolated musl artifacts for both supported Linux architectures by default.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:?usage: build.sh ABSOLUTE_OUTPUT_DIRECTORY [amd64|arm64]}
case "${2-}" in
  '') architectures=(amd64 arm64) ;;
  amd64|arm64) architectures=("$2") ;;
  *) echo 'Expected amd64 or arm64' >&2; exit 2 ;;
esac
mkdir -p "$out"
out=$(cd "$out" && pwd)
case "$out/" in "$repo/"*) echo 'Release artifacts must be outside the worktree' >&2; exit 2;; esac
for architecture in "${architectures[@]}"; do
  case "$architecture" in amd64) platform=linux-x86_64;; arm64) platform=linux-aarch64;; esac
  docker buildx build --platform "linux/$architecture" \
    --file "$repo/rust/release/Dockerfile" --target artifacts \
    --output "type=local,dest=$out/$platform" "$repo"
done
printf 'Unsigned binaries: %s\n' "$out"
