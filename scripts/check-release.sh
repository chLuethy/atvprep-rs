#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
bundle="$root/release"
manifest="$bundle/release.json"
signature="$bundle/release.json.sig"
: "${RELEASE_TRUSTED_KEY:?RELEASE_TRUSTED_KEY must contain the independently trusted OpenSSH Ed25519 public key}"
: "${RELEASE_VERSION:?RELEASE_VERSION must identify the release}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
umask 077
printf '%s\n' "$RELEASE_TRUSTED_KEY" > "$work/release.pub"
cmp "$work/release.pub" "$bundle/release.pub"
printf 'atvprep %s\n' "$RELEASE_TRUSTED_KEY" > "$work/allowed-signers"
ssh-keygen -Y verify -f "$work/allowed-signers" -I atvprep -n atvprep-release -s "$signature" < "$manifest"
jq -e --arg version "$RELEASE_VERSION" '
  (keys | sort) == ["assets", "version"] and .version == $version and
  (.assets | length == 2) and
  ([.assets[].platform] | sort) == ["linux-aarch64", "linux-x86_64"]
' "$manifest" >/dev/null
native=$(uname -m)
for platform in linux-x86_64 linux-aarch64; do
  asset="$bundle/atvprep-$platform"
  test -f "$asset" && test -x "$asset"
  jq -e --arg platform "$platform" --arg sha "$(sha256sum "$asset" | cut -d' ' -f1)" --argjson size "$(wc -c < "$asset")" '
    [.assets[] | select(.platform == $platform and (.url | startswith("https://")) and .sha256 == $sha and .size == $size and (keys | sort) == ["platform", "sha256", "size", "url"])] | length == 1
  ' "$manifest" >/dev/null
  case "$platform" in
    linux-x86_64) readelf -h "$asset" | grep -q 'Advanced Micro Devices X86-64'; arch=x86_64; emulator=qemu-x86_64-static ;;
    linux-aarch64) readelf -h "$asset" | grep -q 'AArch64'; arch=aarch64; emulator=qemu-aarch64-static ;;
  esac
  if readelf -l "$asset" | grep -q 'INTERP'; then echo "$platform is dynamically linked" >&2; exit 1; fi
  # Ubuntu 26.04 dropped qemu-user-static; its qemu-user ships the emulators without the suffix.
  if ! command -v "$emulator" >/dev/null 2>&1; then emulator=${emulator%-static}; fi
  if [ "$native" = "$arch" ]; then runner=(); else runner=("$emulator"); fi
  actual=$("${runner[@]}" "$asset" --version)
  test "$actual" = "atvprep $RELEASE_VERSION"
  "${runner[@]}" "$asset" manager --help >/dev/null
  "${runner[@]}" "$asset" deploy --help >/dev/null
  if [ "$native" = "$arch" ]; then
    mkdir "$work/install"
    "$asset" deploy install --manifest "$manifest" --signature "$signature" --trusted-key "$work/release.pub" --asset-file "$asset" --destination "$work/install/atvprep"
    test "$("$work/install/atvprep" --version)" = "atvprep $RELEASE_VERSION"
  fi
done
case "$native" in x86_64|aarch64) ;; *) echo "Unsupported runner architecture: $native" >&2; exit 1;; esac
printf 'Release %s: signatures, checksums, architectures, CLI and native installation verified\n' "$RELEASE_VERSION"
