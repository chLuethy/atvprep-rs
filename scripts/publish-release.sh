#!/usr/bin/env bash
set -euo pipefail
: "${GITHUB_TOKEN:?}"
: "${GITHUB_REPOSITORY:?}"
: "${GITHUB_SERVER_URL:?}"
: "${RELEASE_VERSION:?}"
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
api="$GITHUB_SERVER_URL/api/v1/repos/$GITHUB_REPOSITORY"
auth="Authorization: token $GITHUB_TOKEN"
tag="v$RELEASE_VERSION"
curl -fsS -H "$auth" "$api/releases?limit=50" > "$work/releases.json"
count=$(jq --arg tag "$tag" '[.[] | select(.tag_name == $tag)] | length' "$work/releases.json")
if [ "$count" -eq 0 ]; then
  body=$(jq -n --arg tag "$tag" '{tag_name:$tag,name:$tag,draft:true,prerelease:($tag | contains("-"))}')
  curl -fsS -H "$auth" -H 'Content-Type: application/json' -d "$body" "$api/releases" > "$work/release.json"
  id=$(jq -er '.id' "$work/release.json")
  echo "Created draft release $id; failures leave it unpublished for inspection."
elif [ "$count" -eq 1 ] && [ "${RESUME_VERIFIED_DRAFT:-}" = true ]; then
  id=$(jq -er --arg tag "$tag" '.[] | select(.tag_name == $tag and .draft == true) | .id' "$work/releases.json")
  echo "Resuming independently verified draft release $id."
else
  echo "Release $tag already exists; refusing to replace it" >&2
  exit 1
fi
curl -fsS -H "$auth" "$api/releases/$id" > "$work/assets.json"
for name in release.json release.json.sig release.pub atvprep-linux-x86_64 atvprep-linux-aarch64; do
  present=$(jq --arg name "$name" '[.assets[] | select(.name == $name)] | length' "$work/assets.json")
  if [ "$present" -eq 0 ]; then
    curl -fsS -H "$auth" -F "attachment=@$root/release/$name" "$api/releases/$id/assets?name=$name" > /dev/null
  elif [ "$present" -ne 1 ] || [ "${RESUME_VERIFIED_DRAFT:-}" != true ]; then
    echo "Unexpected existing asset: $name" >&2; exit 1
  fi
done
curl -fsS -H "$auth" "$api/releases/$id" > "$work/assets.json"
jq -e '(.assets | length) == 5' "$work/assets.json" >/dev/null
for name in release.json release.json.sig release.pub atvprep-linux-x86_64 atvprep-linux-aarch64; do
  size=$(wc -c < "$root/release/$name")
  jq -e --arg name "$name" --argjson size "$size" '[.assets[] | select(.name == $name and .size == $size)] | length == 1' "$work/assets.json" >/dev/null
  echo "Uploaded $name ($size bytes)"
done
curl -fsS -X PATCH -H "$auth" -H 'Content-Type: application/json' -d '{"draft":false}' "$api/releases/$id" > /dev/null
echo "Published $tag"
