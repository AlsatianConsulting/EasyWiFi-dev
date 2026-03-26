#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SESSION_DIR="${1:-}"
TIME_MODE="${2:-any}"

echo "[1/4] cargo fmt --all"
(cd "$ROOT_DIR" && ~/.cargo/bin/cargo fmt --all)

echo "[2/4] cargo check -q"
(cd "$ROOT_DIR" && ~/.cargo/bin/cargo check -q)

echo "[3/4] cargo test -q"
(cd "$ROOT_DIR" && ~/.cargo/bin/cargo test -q)

if [[ -n "$SESSION_DIR" ]]; then
  echo "[4/4] artifact validation skipped: no external validator configured for $SESSION_DIR"
else
  echo "[4/4] artifact validation skipped (no session-dir provided)"
fi

echo "Validation matrix run complete."
