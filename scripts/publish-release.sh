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
body=$(jq -n --arg tag "$tag" '{tag_name:$tag,name:$tag,draft:true,prerelease:($tag | contains("-"))}')
curl -fsS -H "$auth" -H 'Content-Type: application/json' -d "$body" "$api/releases" > "$work/release.json"
id=$(jq -er '.id' "$work/release.json")
echo "Created draft release $id; failures leave it unpublished for inspection."
for name in release.json release.json.sig release.pub atvprep-linux-x86_64 atvprep-linux-aarch64; do
  curl -fsS -H "$auth" -F "attachment=@$root/release/$name" "$api/releases/$id/assets?name=$name" > /dev/null
done
curl -fsS -H "$auth" "$api/releases/$id" > "$work/assets.json"
jq -e '(.assets | length) == 5' "$work/assets.json" >/dev/null
for name in release.json release.json.sig release.pub atvprep-linux-x86_64 atvprep-linux-aarch64; do
  url=$(jq -er --arg name "$name" '[.assets[] | select(.name == $name) | .browser_download_url] | if length == 1 then .[0] else error("asset missing or duplicated") end' "$work/assets.json")
  uuid=$(jq -er --arg name "$name" '.assets[] | select(.name == $name) | .uuid' "$work/assets.json")
  curl -fsS -L -H "$auth" "$GITHUB_SERVER_URL/attachments/$uuid" -o "$work/download-$name"
  cmp "$root/release/$name" "$work/download-$name"
  echo "Verified $name ($url)"
done
curl -fsS -X PATCH -H "$auth" -H 'Content-Type: application/json' -d '{"draft":false}' "$api/releases/$id" > /dev/null
echo "Published $tag"
