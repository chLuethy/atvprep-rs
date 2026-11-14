#!/usr/bin/env bash
# Checks commit subjects against docs/agents/language.md.
#   scripts/check-commit-msg.sh <message-file>   one message (used by .githooks/commit-msg)
#   scripts/check-commit-msg.sh --range A..B     every commit in a range (used by CI)
# Allowed subjects: a Conventional-Commit prefix (`feat:`, `fix(core):`, …), `Merge: …` for own
# merges, and GitHub's own `Merge pull request #…`. Git's default English merge subjects and
# `wip:` are rejected.
set -euo pipefail

types='feat|fix|refactor|docs|chore|test|ci|perf|build|style|revert'
pattern="^(($types)(\([a-z0-9-]+\))?!?: .+|[Mm]erge: .+|Merge pull request #[0-9]+ from .+)$"

failed=0
check() { # <subject> <label>
  case "$1" in
    "fixup! "* | "squash! "* | "amend! "*) return ;;
  esac
  if ! grep -Eq "$pattern" <<<"$1"; then
    echo "Ungültiger Commit-Betreff${2:+ ($2)}: $1" >&2
    failed=1
  fi
}

if [ "${1:-}" = "--range" ]; then
  while IFS=$'\t' read -r sha subject; do
    check "$subject" "${sha:0:7}"
  done < <(git log --format='%H%x09%s' "$2")
else
  subject=$(grep -v '^#' "$1" | sed '/^[[:space:]]*$/d' | head -n1)
  check "$subject" ""
fi

if [ "$failed" -ne 0 ]; then
  cat >&2 <<'EOF'
Erwartet: `<typ>: <deutsche Beschreibung>` mit typ aus feat, fix, refactor, docs, chore, test,
ci, perf, build, style, revert (optional mit Scope, z. B. `fix(core):`), oder `Merge: <deutsch>`.
Siehe docs/agents/language.md.
EOF
  exit 1
fi
