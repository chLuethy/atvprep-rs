#!/usr/bin/env bash
# The Rust gate, also run by CI. The full gate (Rust, frontend, E2E) is scripts/check-all.sh.
set -euo pipefail
cd "$(dirname "$0")"

step() { printf '\n== rust: %s\n' "$1"; }
on_exit() {
  local status=$?
  if [ "$status" -ne 0 ]; then
    printf '\n== rust: FEHLER in Schritt "%s" (exit %s)\n' "$current" "$status" >&2
  fi
}
current=start
trap on_exit EXIT

current=fmt; step "$current"
cargo fmt --all -- --check
current=clippy; step "$current"
cargo clippy --locked --workspace --all-targets -- -D warnings
current=test; step "$current"
cargo test --locked --workspace
current=deny; step "$current"
cargo deny check
