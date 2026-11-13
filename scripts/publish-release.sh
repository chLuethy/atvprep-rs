#!/usr/bin/env bash
# Publish the verified bytes in release/ as the GitHub release v$RELEASE_VERSION (ADR 0005, ADR 0009).
# The release is created as a draft, its uploaded bytes are downloaded again and compared, and only
# then it is published. A leftover draft from a failed run was never public and is replaced.
set -euo pipefail
: "${GH_TOKEN:?}"
: "${GITHUB_REPOSITORY:?}"
: "${RELEASE_VERSION:?}"
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export GH_REPO=$GITHUB_REPOSITORY
tag="v$RELEASE_VERSION"
names=(release.json release.json.sig release.pub atvprep-linux-x86_64 atvprep-linux-aarch64)

existing=$(gh api "repos/$GH_REPO/releases?per_page=100" --jq \
  "[.[] | select(.tag_name == \"$tag\")] | if length == 0 then \"none\" elif length == 1 and .[0].draft then \"draft \(.[0].id)\" else \"published\" end")
case "$existing" in
  none) ;;
  draft\ *)
    echo "Deleting leftover unpublished draft ${existing#draft }."
    gh api -X DELETE "repos/$GH_REPO/releases/${existing#draft }" ;;
  *) echo "Release $tag already exists; refusing to replace it" >&2; exit 1 ;;
esac

prerelease=()
case "$tag" in *-*) prerelease=(--prerelease) ;; esac
files=()
for name in "${names[@]}"; do files+=("$root/release/$name"); done
gh release create "$tag" --verify-tag --draft --title "$tag" --notes "" "${prerelease[@]}" "${files[@]}"
echo "Created draft $tag; failures leave it unpublished for inspection."

test "$(gh release view "$tag" --json assets --jq '.assets | length')" -eq "${#names[@]}"
gh release download "$tag" --dir "$work"
for name in "${names[@]}"; do
  cmp "$root/release/$name" "$work/$name"
  echo "Uploaded $name ($(wc -c < "$work/$name") bytes), downloaded bytes identical"
done

gh release edit "$tag" --draft=false
echo "Published $tag"
