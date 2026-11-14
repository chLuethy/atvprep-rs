#!/usr/bin/env bash
# The full gate before a commit or merge: Rust, frontend and the Playwright journeys.
# Runs from any worktree; installs the frontend dependencies when they are missing.
# `--no-e2e` skips the Playwright journeys (they build the SPA and the server, then start the
# server on 127.0.0.1:8765 themselves).
set -euo pipefail
cd "$(dirname "$0")/.."

e2e=1
[ "${1:-}" = "--no-e2e" ] && e2e=0

step() { printf '\n== %s\n' "$1"; }

./rust/check.sh

step "frontend: npm ci"
if [ ! -d frontend/node_modules ]; then
  (cd frontend && npm ci)
else
  echo "node_modules vorhanden, übersprungen"
fi
step "frontend: typecheck"
(cd frontend && npm run typecheck)
step "frontend: test"
(cd frontend && npm test)
step "frontend: build"
(cd frontend && npm run build)

if [ "$e2e" -eq 1 ]; then
  step "e2e: scripts/run-e2e.sh"
  ./scripts/run-e2e.sh
fi

step "alles grün"
