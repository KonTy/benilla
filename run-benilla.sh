#!/usr/bin/env bash
# Start the local VMaNGOS stack, wait for it, then run the client.
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
start_server=1
if [ "${1:-}" = "--no-server" ]; then
    start_server=0
    shift
fi
if [ "$#" -gt 2 ]; then
    echo "Usage: ./run-benilla.sh [--no-server] [user [password]]" >&2
    exit 1
fi
if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    echo "Usage: ./run-benilla.sh [--no-server] [user [password]]"
    echo "Set WOW_DATA to your own install's Data folder and WOW_HOST to your server."
    echo "Without credentials, use the login screen. Builds or updates the release client."
    echo "Starts ../vmangos/bin-db/start-server.sh for a local WOW_HOST by default."
    echo "BENILLA_SERVER_START overrides that script; --no-server skips server startup."
    exit 0
fi

# Preserve the original project layout as an optional fallback, not a fixed home path.
if [ "${WOW_DATA+x}" != x ] && [ -d "$ROOT/../VanillaClient/Data" ]; then
    export WOW_DATA="$ROOT/../VanillaClient/Data"
fi
export WOW_HOST="${WOW_HOST:-127.0.0.1:3724}"
user="${1:-${WOW_USER:-}}"
pass="${2:-${WOW_PASS:-}}"
if [ -n "$user" ] && [ -n "$pass" ]; then
    export WOW_USER="$user" WOW_PASS="$pass"
else
    # Presence of both variables, even empty, opts the client into automated login.
    unset WOW_USER WOW_PASS
fi

case "$WOW_HOST" in
    localhost|localhost:*|127.0.0.1|127.0.0.1:*|'[::1]'|'[::1]:'*) ;;
    *) start_server=0 ;;
esac
if [ "$start_server" -eq 1 ]; then
    server_start="${BENILLA_SERVER_START:-$ROOT/../vmangos/bin-db/start-server.sh}"
    if [ ! -x "$server_start" ]; then
        echo "run-benilla: server starter not found or not executable: $server_start" >&2
        echo "Set BENILLA_SERVER_START to your local stack's script, or use --no-server." >&2
        exit 1
    fi
    command -v python3 >/dev/null || {
        echo "run-benilla: python3 is required to check server readiness; run ./build-benilla.sh" >&2
        exit 1
    }
    "$server_start"
    python3 - "$WOW_HOST" "${BENILLA_WORLD_PORT:-8085}" <<'PY'
import socket
import sys
import time

target, world_port = sys.argv[1:]
host, separator, auth_port = target.rpartition(":")
if not separator or target == "[::1]":
    host, auth_port = target, "3724"
host = host.strip("[]")
try:
    ports = (int(auth_port), int(world_port))
    if any(not 1 <= port <= 65535 for port in ports):
        raise ValueError("ports must be in 1..65535")
except ValueError as error:
    sys.exit(f"run-benilla: invalid server port: {error}")

deadline = time.monotonic() + 60
while True:
    waiting = []
    for port in ports:
        try:
            with socket.create_connection((host, port), timeout=0.5):
                pass
        except OSError:
            waiting.append(port)
    if not waiting:
        print(f"run-benilla: auth and world servers ready on {host}, ports {ports}")
        break
    if time.monotonic() >= deadline:
        sys.exit(f"run-benilla: server not ready after 60s; {host} ports {waiting} unavailable")
    time.sleep(0.5)
PY
fi

cd "$ROOT"
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
# Cargo resolves custom target directories, rebuilds stale code and propagates failures.
exec cargo run --locked --release -p benilla
