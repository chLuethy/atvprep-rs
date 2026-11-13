#!/bin/sh
# Installs or updates atvprep from the latest signed release.
#
#   curl -fsSL https://raw.githubusercontent.com/chLuethy/atvprep-rs/main/scripts/install.sh | sudo sh
#   ... | sudo sh -s -- servarr sonarr /pfad/zu/sonarr-config/atvprep-hook
#
# This script itself is not trusted: it only runs binaries whose signed manifest
# verifies against the root-owned trust key (ADR 0003). On the first install the
# key is fetched once (trust on first use) and its fingerprint is printed; set
# ATVPREP_KEY_FINGERPRINT to pin it. Later runs never fetch the key again.
#
# Environment:
#   ATVPREP_VERSION          release to install instead of the latest, e.g. 0.5.5
#   ATVPREP_DEST             binary path (default /usr/local/bin/atvprep)
#   ATVPREP_TRUSTED_KEY      trust key path (default /etc/atvprep/release.pub)
#   ATVPREP_KEY_FINGERPRINT  expected SHA256:... fingerprint of the key
#   ATVPREP_HOOKS            hook register (default /etc/atvprep/hooks)
#   ATVPREP_HOST             release host (default https://github.com; the Gitea
#                            mirror is https://git.ch-luethy.ch, ADR 0009)
#   ATVPREP_REPO             owner/repo on that host (default chLuethy/atvprep-rs
#                            on GitHub, chl/atvprep-rs elsewhere)
set -eu

HOST=${ATVPREP_HOST:-https://github.com}
HOST=${HOST%/}
case "$HOST" in
    https://github.com)
        REPO=${ATVPREP_REPO:-chLuethy/atvprep-rs}
        LATEST_URL=https://api.github.com/repos/$REPO/releases/latest
        RAW_URL=https://raw.githubusercontent.com/$REPO/main
        ;;
    *)
        REPO=${ATVPREP_REPO:-chl/atvprep-rs}
        LATEST_URL=$HOST/api/v1/repos/$REPO/releases/latest
        RAW_URL=$HOST/$REPO/raw/branch/main
        ;;
esac
DEST=${ATVPREP_DEST:-/usr/local/bin/atvprep}
KEY=${ATVPREP_TRUSTED_KEY:-/etc/atvprep/release.pub}
HOOKS=${ATVPREP_HOOKS:-/etc/atvprep/hooks}

die() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}

for tool in curl ssh-keygen sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool fehlt; bitte curl, openssh-client (ssh-keygen) und coreutils (sha256sum) installieren"
done
[ "$(id -u)" -eq 0 ] || [ "${ATVPREP_ALLOW_NONROOT:-}" = 1 ] || die "bitte mit sudo ausführen"

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) PLATFORM=linux-x86_64 ;;
    Linux-aarch64 | Linux-arm64) PLATFORM=linux-aarch64 ;;
    *) die "keine Release-Plattform für $(uname -s) $(uname -m); Releases gibt es nur für Linux x86_64 und aarch64" ;;
esac

MODE=update
case "${1:-}" in
    "") ;;
    servarr)
        [ $# -eq 3 ] || die "Aufruf: install.sh servarr <sonarr|radarr|whisparr> <pfad>/atvprep-hook"
        case "$2" in sonarr | radarr | whisparr) ;; *) die "unbekannte Anwendung: $2" ;; esac
        [ "$(basename "$3")" = atvprep-hook ] || die "der Zielname muss atvprep-hook sein"
        MODE=servarr
        APP=$2
        HOOK_PATH=$3
        ;;
    *) die "unbekanntes Argument: $1" ;;
esac

umask 077
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fetch() { curl -fsSL --proto '=https' --max-time 300 -o "$2" "$1"; }

if [ -n "${ATVPREP_VERSION:-}" ]; then
    TAG=v${ATVPREP_VERSION#v}
else
    fetch "$LATEST_URL" "$WORK/latest.json"
    TAG=$(sed -n 's/.*"tag_name": *"\(v[0-9][0-9.]*\)".*/\1/p' "$WORK/latest.json" | head -n 1)
    [ -n "$TAG" ] || die "neueste Version konnte nicht ermittelt werden"
fi
VERSION=${TAG#v}
BASE=$HOST/$REPO/releases/download/$TAG

fetch "$BASE/release.json" "$WORK/release.json"
fetch "$BASE/release.json.sig" "$WORK/release.json.sig"

FRESH_KEY=0
if [ ! -f "$KEY" ]; then
    fetch "$BASE/release.pub" "$WORK/release.pub"
    [ "$(wc -l < "$WORK/release.pub")" -le 1 ] && grep -q '^ssh-ed25519 ' "$WORK/release.pub" || die "release.pub ist kein einzelner OpenSSH-Ed25519-Schlüssel"
    FINGERPRINT=$(ssh-keygen -lf "$WORK/release.pub" | awk '{print $2}')
    printf 'Erstinstallation: Vertrauensschlüssel %s wird nach %s übernommen.\n' "$FINGERPRINT" "$KEY"
    printf 'Fingerabdruck mit dem ausserhalb von GitHub und Gitea notierten vergleichen!\n'
    if [ -n "${ATVPREP_KEY_FINGERPRINT:-}" ] && [ "$ATVPREP_KEY_FINGERPRINT" != "$FINGERPRINT" ]; then
        die "Fingerabdruck stimmt nicht mit ATVPREP_KEY_FINGERPRINT überein"
    fi
    FRESH_KEY=1
    KEY_CANDIDATE=$WORK/release.pub
else
    KEY_CANDIDATE=$KEY
fi

printf 'atvprep %s\n' "$(sed -n '1p' "$KEY_CANDIDATE")" > "$WORK/allowed-signers"
ssh-keygen -Y verify -f "$WORK/allowed-signers" -I atvprep -n atvprep-release \
    -s "$WORK/release.json.sig" < "$WORK/release.json" >/dev/null 2>&1 \
    || die "Signatur des Manifests ist ungültig"

MANIFEST_VERSION=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$WORK/release.json" | head -n 1)
[ "$MANIFEST_VERSION" = "$VERSION" ] || die "Manifest-Version $MANIFEST_VERSION passt nicht zum Release $VERSION"
EXPECTED=$(awk -v platform="$PLATFORM" '
    /"platform"/ { match_platform = index($0, "\"" platform "\"") > 0 }
    match_platform && /"sha256"/ { gsub(/.*"sha256": *"|".*/, ""); print; exit }
' "$WORK/release.json")
case "$EXPECTED" in *[!0-9a-f]* | "") die "kein Asset für $PLATFORM im Manifest" ;; esac
[ "${#EXPECTED}" -eq 64 ] || die "ungültige Prüfsumme im Manifest"

ASSET=$WORK/atvprep-$PLATFORM
fetch "$BASE/atvprep-$PLATFORM" "$ASSET"
[ "$(sha256sum "$ASSET" | awk '{print $1}')" = "$EXPECTED" ] || die "SHA-256 des Downloads stimmt nicht mit dem signierten Manifest überein"
chmod 0755 "$ASSET"

if [ "$FRESH_KEY" -eq 1 ]; then
    mkdir -p "$(dirname "$KEY")"
    chmod 0755 "$(dirname "$KEY")"
    cp "$KEY_CANDIDATE" "$KEY"
    chmod 0644 "$KEY"
fi

deploy() {
    "$ASSET" deploy "$@" --manifest "$WORK/release.json" --signature "$WORK/release.json.sig" \
        --trusted-key "$KEY" --asset-file "$ASSET"
}

INSTALLED_NEW=0
if [ ! -e "$DEST" ]; then
    mkdir -p "$(dirname "$DEST")"
    deploy install --destination "$DEST" >/dev/null
    INSTALLED_NEW=1
    printf 'atvprep %s installiert nach %s\n' "$VERSION" "$DEST"
else
    CURRENT=$("$DEST" --version | awk '{print $NF}')
    if [ "$CURRENT" = "$VERSION" ]; then
        printf 'atvprep %s ist bereits aktuell\n' "$VERSION"
    elif [ "$(printf '%s\n%s\n' "$CURRENT" "$VERSION" | sort -V | tail -n 1)" = "$CURRENT" ]; then
        die "installiert ist $CURRENT, neuer als $VERSION; ein Downgrade ist nur mit 'atvprep deploy rollback'"
    else
        deploy update --destination "$DEST" >/dev/null
        printf 'atvprep %s -> %s aktualisiert\n' "$CURRENT" "$VERSION"
    fi
fi

BIN_DIR=$(dirname "$DEST")
for alias in atvprep-manager atvprep-hook; do
    link=$BIN_DIR/$alias
    if [ -L "$link" ] || [ ! -e "$link" ]; then
        ln -sf "$DEST" "$link"
    else
        printf 'Warnung: %s ist keine Verknüpfung und bleibt unverändert\n' "$link" >&2
    fi
done

install_hook() {
    deploy servarr --application "$1" --destination "$2" >/dev/null
}

if [ "$MODE" = servarr ]; then
    install_hook "$APP" "$HOOK_PATH" || die "Hook für $APP konnte nicht nach $HOOK_PATH installiert werden"
    mkdir -p "$(dirname "$HOOKS")"
    touch "$HOOKS"
    grep -vF " $HOOK_PATH" "$HOOKS" > "$WORK/hooks" || true
    printf '%s %s\n' "$APP" "$HOOK_PATH" >> "$WORK/hooks"
    cp "$WORK/hooks" "$HOOKS"
    chmod 0644 "$HOOKS"
    printf 'Hook %s installiert und in %s eingetragen\n' "$HOOK_PATH" "$HOOKS"
fi

FAILED=""
if [ -s "$HOOKS" ]; then
    while read -r app path; do
        [ -n "$app" ] || continue
        [ "$MODE" = servarr ] && [ "$path" = "$HOOK_PATH" ] && continue
        if install_hook "$app" "$path"; then
            printf 'Hook %s (%s) auf %s\n' "$path" "$app" "$VERSION"
        else
            printf 'Warnung: Hook %s (%s) konnte nicht aktualisiert werden\n' "$path" "$app" >&2
            FAILED="$FAILED $path"
        fi
    done < "$HOOKS"
elif [ "$INSTALLED_NEW" -eq 1 ] && [ "$MODE" != servarr ]; then
    cat <<EOF

Servarr-Hook einrichten (einmalig, je Anwendung):
  curl -fsSL $RAW_URL/scripts/install.sh | sudo sh -s -- servarr sonarr /pfad/zu/sonarr-config/atvprep-hook
Danach aktualisiert jeder erneute Lauf dieses Skripts auch die eingetragenen Hooks.
Auf dem produktiven Host nur im Releasefenster, siehe DEPLOYMENT.md.
EOF
fi

[ -z "$FAILED" ] || die "fehlgeschlagene Hooks:$FAILED"
