#!/usr/bin/env bash
# Run against a 1.12.1 server; the sibling VMaNGOS stack must be started separately.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "$SCRIPT_DIR/Cargo.toml" ]; then
    ROOT="$SCRIPT_DIR"
elif [ -f "$SCRIPT_DIR/benilla/Cargo.toml" ]; then
    ROOT="$SCRIPT_DIR/benilla"
else
    echo "run-benilla: expected the repository here or in ./benilla" >&2
    exit 1
fi
if [ "$#" -gt 2 ]; then
    echo "Usage: ./run-benilla.sh [user [password]]" >&2
    exit 1
fi
if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    echo "Usage: ./run-benilla.sh [user [password]]"
    echo "Set WOW_DATA to your own install's Data folder and WOW_HOST to your server."
    echo "Without credentials, use the login screen. Builds or updates the release client."
    exit 0
fi

# Preserve the original project layout as an optional fallback, not a fixed home path.
if [ "${WOW_DATA+x}" != x ] && [ -d "$ROOT/../VanillaClient/Data" ]; then
    export WOW_DATA="$ROOT/../VanillaClient/Data"
fi
export WOW_HOST="${WOW_HOST:-127.0.0.1:3724}"
export WOW_USER="${1:-${WOW_USER:-}}"
export WOW_PASS="${2:-${WOW_PASS:-}}"

cd "$ROOT"
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
# Cargo resolves custom target directories, rebuilds stale code and propagates failures.
exec cargo run --locked --release -p benilla
