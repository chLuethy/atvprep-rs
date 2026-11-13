#!/usr/bin/env bash
# Mirror the published GitHub release v$RELEASE_VERSION to Gitea (ADR 0009).
# Usage: scripts/mirror-release.sh ASSET_DIR   (ASSET_DIR holds the five files downloaded from GitHub)
#
# Checks the downloaded files against the signed manifest at the tag before anything goes to Gitea,
# pushes the tag and its commit as Gitea `main` without force, uploads the assets as a draft,
# publishes it and compares the public download bytes. Re-running after success is a no-op.
set -euo pipefail
: "${GITEA_TOKEN:?}"
: "${GITEA_URL:?}"
: "${GITEA_REPO:?}"
: "${RELEASE_TRUSTED_KEY:?}"
: "${RELEASE_VERSION:?}"
GITEA_URL=${GITEA_URL%/}
case "$GITEA_URL" in https://*) ;; *) echo "GITEA_URL must start with https://" >&2; exit 1 ;; esac
assets=$(cd "${1:?usage: mirror-release.sh ASSET_DIR}" && pwd)
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tag="v$RELEASE_VERSION"
names=(release.json release.json.sig release.pub atvprep-linux-x86_64 atvprep-linux-aarch64)
api="$GITEA_URL/api/v1/repos/$GITEA_REPO"
auth="Authorization: token $GITEA_TOKEN"

# 1. The bytes to mirror are the signed release: manifest, signature and key as committed at the
#    tag, signature valid for the trusted key, binaries matching the manifest's hashes and sizes.
for name in release.json release.json.sig release.pub; do
  git -C "$root" show "$tag:release/$name" | cmp - "$assets/$name"
done
printf 'atvprep %s\n' "$RELEASE_TRUSTED_KEY" > "$work/allowed-signers"
ssh-keygen -Y verify -f "$work/allowed-signers" -I atvprep -n atvprep-release \
  -s "$assets/release.json.sig" < "$assets/release.json"
test "$(jq -er '.version' "$assets/release.json")" = "$RELEASE_VERSION"
for platform in linux-x86_64 linux-aarch64; do
  file="$assets/atvprep-$platform"
  jq -e --arg p "$platform" --arg sha "$(sha256sum "$file" | cut -d' ' -f1)" --argjson size "$(wc -c < "$file")" \
    '[.assets[] | select(.platform == $p and .sha256 == $sha and .size == $size)] | length == 1' \
    "$assets/release.json" > /dev/null
done
echo "Assets match the signed manifest of $tag"

# 2. Code: the tagged commit becomes Gitea main. No force: a diverged mirror stops the run.
remote="https://oauth2:$GITEA_TOKEN@${GITEA_URL#https://}/$GITEA_REPO.git"
git -C "$root" push "$remote" "$(git -C "$root" rev-parse "$tag^{commit}"):refs/heads/main" "refs/tags/$tag"

# 3. Release.
verify_public() {
  for name in "${names[@]}"; do
    curl -fsSL --proto '=https' -o "$work/$name" "$GITEA_URL/$GITEA_REPO/releases/download/$tag/$name"
    cmp "$assets/$name" "$work/$name"
  done
}
curl -fsS -H "$auth" "$api/releases?limit=50" > "$work/releases.json"
existing=$(jq -r --arg tag "$tag" \
  '[.[] | select(.tag_name == $tag)] | if length == 0 then "none" elif length == 1 and .[0].draft then "draft \(.[0].id)" elif length == 1 then "published" else "several" end' \
  "$work/releases.json")
case "$existing" in
  none) ;;
  draft\ *)
    echo "Deleting leftover unpublished Gitea draft ${existing#draft }."
    curl -fsS -X DELETE -H "$auth" "$api/releases/${existing#draft }" ;;
  published)
    verify_public
    echo "Gitea already has $tag with identical bytes; nothing to do."
    exit 0 ;;
  *) echo "Unexpected Gitea releases for $tag" >&2; exit 1 ;;
esac

body=$(jq -n --arg tag "$tag" '{tag_name:$tag,name:$tag,draft:true,prerelease:($tag | contains("-"))}')
id=$(curl -fsS -H "$auth" -H 'Content-Type: application/json' -d "$body" "$api/releases" | jq -er '.id')
echo "Created Gitea draft $id"
for name in "${names[@]}"; do
  curl -fsS -H "$auth" -F "attachment=@$assets/$name" "$api/releases/$id/assets?name=$name" > /dev/null
done
curl -fsS -H "$auth" "$api/releases/$id" > "$work/release.json"
jq -e --argjson count "${#names[@]}" '(.assets | length) == $count' "$work/release.json" > /dev/null
for name in "${names[@]}"; do
  jq -e --arg name "$name" --argjson size "$(wc -c < "$assets/$name")" \
    '[.assets[] | select(.name == $name and .size == $size)] | length == 1' "$work/release.json" > /dev/null
done
curl -fsS -X PATCH -H "$auth" -H 'Content-Type: application/json' -d '{"draft":false}' "$api/releases/$id" > /dev/null
verify_public
echo "Mirrored $tag to $GITEA_URL/$GITEA_REPO"
