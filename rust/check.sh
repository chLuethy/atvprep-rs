#!/usr/bin/env bash
# The one gate: nothing is committed while this is red.
set -euo pipefail
cd "$(dirname "$0")"
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
