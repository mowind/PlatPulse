#!/usr/bin/env bash
# Start an installed PlatPulse Server against disposable state and exercise backup.
set -euo pipefail
SERVER="${1:-/usr/bin/platpulse-server}"
WEB_ROOT="${PLATPULSE_INSTALLED_WEB_ROOT:-/usr/share/platpulse/web}"
[[ -x "$SERVER" ]] || { echo "installed Server is not executable: $SERVER" >&2; exit 2; }
[[ -f "$WEB_ROOT/index.html" ]] || { echo "installed WebUI is missing: $WEB_ROOT" >&2; exit 2; }
command -v curl >/dev/null 2>&1 || { echo 'curl is required for installed Server smoke' >&2; exit 2; }
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/platpulse-installed-smoke.XXXXXX")"
SERVER_PID=""
cleanup() {
  if [[ -n "$SERVER_PID" ]]; then kill "$SERVER_PID" >/dev/null 2>&1 || true; wait "$SERVER_PID" 2>/dev/null || true; fi
  rm -rf "$RUN_ROOT"
}
trap cleanup EXIT
PORT=$((20000 + ($$ % 20000)))
mkdir -p "$RUN_ROOT/state" "$RUN_ROOT/backups"
CONFIG="$RUN_ROOT/server.toml"
cat > "$CONFIG" <<EOF
state_dir = "$RUN_ROOT/state"
db_path = "$RUN_ROOT/state/platpulse.db"
backup_dir = "$RUN_ROOT/backups"
pepper_file = "$RUN_ROOT/state/server-pepper"
web_root = "$WEB_ROOT"
listen = "127.0.0.1:$PORT"
public_base_url = "http://127.0.0.1:$PORT"
development = true
EOF
"$SERVER" init --config "$CONFIG" >/dev/null
"$SERVER" serve --config "$CONFIG" >"$RUN_ROOT/server.log" 2>&1 &
SERVER_PID=$!
status=""
for _ in $(seq 1 100); do
  kill -0 "$SERVER_PID" 2>/dev/null || { cat "$RUN_ROOT/server.log" >&2; exit 1; }
  status="$(curl -sS --connect-timeout 1 --max-time 2 -o "$RUN_ROOT/live.json" -w '%{http_code}' "http://127.0.0.1:$PORT/health/live" 2>/dev/null || true)"
  [[ "$status" == 200 ]] && break
  sleep 0.1
done
[[ "$status" == 200 ]] || { cat "$RUN_ROOT/server.log" >&2; echo 'installed Server did not become live' >&2; exit 1; }
curl -fsS "http://127.0.0.1:$PORT/" | grep -qi '<!doctype html'
"$SERVER" backup --config "$CONFIG" >/dev/null
find "$RUN_ROOT/backups" -type f -print -quit | grep -q .
test -f "$RUN_ROOT/state/platpulse.db"
printf 'installed Server state and backup smoke: PASS\n'
