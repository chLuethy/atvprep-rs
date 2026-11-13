#!/usr/bin/env bash
# Write release/release.json for the binaries a local `rust/release/build.sh` produced.
# Usage: scripts/make-manifest.sh BUILD_DIR   (BUILD_DIR holds linux-x86_64/atvprep and linux-aarch64/atvprep)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
build=${1:?usage: make-manifest.sh BUILD_DIR}
version=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\([^"]*\)"/\1/p;}' "$root/rust/Cargo.toml")
: "${version:?cannot read the workspace version}"
base=${RELEASE_BASE_URL:-https://github.com/chLuethy/atvprep-rs/releases/download/v$version}
entries=()
for platform in linux-x86_64 linux-aarch64; do
  asset="$build/$platform/atvprep"
  test -f "$asset"
  entries+=("$(jq -n --arg platform "$platform" --arg url "$base/atvprep-$platform" \
    --arg sha "$(shasum -a 256 "$asset" | cut -d' ' -f1)" --argjson size "$(wc -c < "$asset" | tr -d ' ')" \
    '{platform:$platform,url:$url,sha256:$sha,size:$size}')")
done
printf '%s\n' "${entries[@]}" | jq -s --arg version "$version" '{version:$version,assets:.}' > "$root/release/release.json"
cat "$root/release/release.json"
echo "Sign it next: ssh-keygen -Y sign -f /path/to/private-key -n atvprep-release release/release.json" >&2
