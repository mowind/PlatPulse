#!/usr/bin/env bash
# Rebuild the local PlatPulse artifacts and restart the user-level systemd
# services so the running Server serves the WebUI build that is on disk.
#
# Why the restart is not optional: the Server reads dist/index.html once at
# startup and serves the hashed assets beside it from disk. Rebuilding the WebUI
# replaces those hashed files, so a Server left running keeps handing out an
# index.html that references assets which no longer exist and the browser
# renders a blank page. This script rebuilds, restarts whenever anything the
# running services depend on changed, and then proves the live Server resolves
# every asset the current build references.
#
# It never touches the SQLite state, the Owner account, the enrollment
# credential, the pepper, or the service configuration.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_DIR="${HOME}/.config/platpulse"
SERVER_UNIT="platpulse-server.service"
AGENT_UNIT="platpulse-agent.service"
LISTEN=""
TIMEOUT=60
SKIP_TESTS=0
SKIP_WEB=0
SKIP_RUST=0
RESTART_AGENT=1
FORCE=0
CHECK_ONLY=0
STAGING=""
LOG=""
WEB_CHANGED=0
RUST_CHANGED=0

usage() {
  cat >&2 <<'EOF'
usage: scripts/deploy-local.sh [options]

Rebuild the local Agent/Server binaries and the WebUI, then restart the
user-level systemd services so the running Server serves the build on disk.

Options:
  --check-only        Report whether the live Server serves the current WebUI
                      build and exit; build nothing, restart nothing
  --skip-tests        Skip WebUI lint, typecheck, and unit tests
  --skip-web          Do not rebuild the WebUI
  --skip-rust         Do not rebuild the Rust release binaries
  --no-agent          Leave the Agent service untouched
  --force             Restart the services even when nothing changed
  --timeout <sec>     Readiness timeout in seconds (default: 60)
  --listen <addr>     Server address to probe (default: server.toml listen)
  --config-dir <dir>  PlatPulse configuration directory
                      (default: ~/.config/platpulse)
  -h, --help          Print this message
EOF
  exit 2
}

fail() {
  printf 'deploy failed: %s\n' "$1" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check-only) CHECK_ONLY=1; shift ;;
    --skip-tests) SKIP_TESTS=1; shift ;;
    --skip-web) SKIP_WEB=1; shift ;;
    --skip-rust) SKIP_RUST=1; shift ;;
    --no-agent) RESTART_AGENT=0; shift ;;
    --force) FORCE=1; shift ;;
    --timeout) TIMEOUT="${2:-}"; shift 2 ;;
    --listen) LISTEN="${2:-}"; shift 2 ;;
    --config-dir) CONFIG_DIR="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done
[[ "$TIMEOUT" =~ ^[0-9]+$ ]] || usage
[[ -n "$CONFIG_DIR" ]] || usage

SERVER_CONFIG="$CONFIG_DIR/server.toml"
AGENT_CONFIG="$CONFIG_DIR/agent.toml"
WEB_DIR="$ROOT/platpulse-web"
DIST="$WEB_DIR/dist"
SERVER_BIN="$ROOT/target/release/platpulse-server"
AGENT_BIN="$ROOT/target/release/platpulse-agent"

cleanup() {
  [[ -z "$STAGING" ]] || rm -rf "$STAGING"
  [[ -z "$LOG" ]] || rm -f "$LOG"
}
trap cleanup EXIT

# systemd --user needs the user bus; a non-interactive shell often lacks it.
ensure_user_bus() {
  local uid
  uid="$(id -u)"
  if [[ -z "${XDG_RUNTIME_DIR:-}" && -d "/run/user/$uid" ]]; then
    export XDG_RUNTIME_DIR="/run/user/$uid"
  fi
  if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" && -n "${XDG_RUNTIME_DIR:-}" ]]; then
    export DBUS_SESSION_BUS_ADDRESS="unix:path=${XDG_RUNTIME_DIR}/bus"
  fi
}

# --- preconditions -----------------------------------------------------------

[[ -r "$SERVER_CONFIG" ]] || fail "missing Server configuration: $SERVER_CONFIG"
[[ -r "$AGENT_CONFIG" ]] || fail "missing Agent configuration: $AGENT_CONFIG"
command -v curl >/dev/null 2>&1 || fail 'curl is required'
ensure_user_bus
systemctl --user cat "$SERVER_UNIT" >/dev/null 2>&1 \
  || fail "user unit not installed: $SERVER_UNIT"

if [[ -z "$LISTEN" ]]; then
  LISTEN="$(sed -n 's/^[[:space:]]*listen[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$SERVER_CONFIG" | head -n1)"
fi
[[ -n "$LISTEN" ]] || LISTEN="127.0.0.1:8080"
PROBE_HOST="${LISTEN%:*}"
PROBE_PORT="${LISTEN##*:}"
case "$PROBE_HOST" in
  0.0.0.0|::|"[::]"|'') PROBE_HOST="127.0.0.1" ;;
esac
BASE_URL="http://${PROBE_HOST}:${PROBE_PORT}"

SERVER_EXEC="$(systemctl --user cat "$SERVER_UNIT" 2>/dev/null | sed -n 's/^ExecStart=\([^ ]*\).*/\1/p' | head -n1)"

# --- helpers -----------------------------------------------------------------

dist_fingerprint() {
  [[ -f "$DIST/index.html" ]] || { printf 'missing'; return 0; }
  find "$DIST" -type f -print0 | LC_ALL=C sort -z \
    | xargs -0 sha256sum | sha256sum | cut -c1-16
}

asset_refs() {
  grep -oE '/assets/[A-Za-z0-9._-]+' "$1" 2>/dev/null | LC_ALL=C sort -u || true
}

# 0 when the live Server answers with the current build and every asset it
# references resolves to the bytes on disk; 1 otherwise (including unreachable).
server_serves_current_assets() {
  local live_index live_refs disk_refs ref code disk_hash live_hash
  disk_refs="$(asset_refs "$DIST/index.html")"
  [[ -n "$disk_refs" ]] || return 1
  live_index="$(curl -fsS --max-time 5 "$BASE_URL/" 2>/dev/null)" || return 1
  live_refs="$(printf '%s' "$live_index" | grep -oE '/assets/[A-Za-z0-9._-]+' | LC_ALL=C sort -u || true)"
  [[ "$live_refs" == "$disk_refs" ]] || return 1
  while IFS= read -r ref; do
    [[ -n "$ref" ]] || continue
    code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$BASE_URL$ref" 2>/dev/null || printf '000')"
    [[ "$code" == 200 ]] || return 1
    disk_hash="$(sha256sum "$DIST$ref" | cut -d' ' -f1)"
    live_hash="$(curl -fsS --max-time 30 "$BASE_URL$ref" 2>/dev/null | sha256sum | cut -d' ' -f1)"
    [[ "$disk_hash" == "$live_hash" ]] || return 1
  done <<<"$disk_refs"
  return 0
}

run_step() {
  local label="$1"; shift
  printf '  %-26s' "$label"
  if "$@" >>"$LOG" 2>&1; then
    printf 'ok\n'
  else
    printf 'FAILED\n'
    tail -n 40 "$LOG" >&2
    fail "$label"
  fi
}

web_lint_typecheck_test() {
  ( cd "$WEB_DIR" && npm run lint && npm run typecheck && npm test )
}

web_build_staging() {
  ( cd "$WEB_DIR" && npx vite build --outDir "$STAGING" --emptyOutDir )
}

wait_ready() {
  local i body
  READY_BODY=""
  for ((i = 0; i < TIMEOUT; i++)); do
    if body="$(curl -fsS --max-time 3 "$BASE_URL/health/ready" 2>/dev/null)"; then
      READY_BODY="$body"
      return 0
    fi
    sleep 1
  done
  return 1
}

# --- check-only --------------------------------------------------------------

if [[ "$CHECK_ONLY" -eq 1 ]]; then
  printf 'Server:   %s\n' "$BASE_URL"
  printf 'WebUI:    %s (fingerprint %s)\n' "$DIST" "$(dist_fingerprint)"
  if server_serves_current_assets; then
    printf 'Verdict:  live Server serves the current WebUI build\n'
    exit 0
  fi
  printf 'Verdict:  STALE - the live Server does not serve the current WebUI build\n' >&2
  printf '          run scripts/deploy-local.sh to rebuild and restart\n' >&2
  exit 1
fi

# --- build -------------------------------------------------------------------

LOG="$(mktemp "${TMPDIR:-/tmp}/platpulse-deploy.XXXXXX")"

printf 'Repository: %s\n' "$ROOT"
printf 'HEAD:       %s\n' "$(git -C "$ROOT" log -1 --format='%h %s' 2>/dev/null || printf 'unknown')"

RUST_BEFORE=""
if [[ -x "$SERVER_BIN" && -x "$AGENT_BIN" ]]; then
  RUST_BEFORE="$(sha256sum "$SERVER_BIN" "$AGENT_BIN" | sha256sum | cut -c1-16)"
fi
WEB_BEFORE="$(dist_fingerprint)"

echo
echo 'Build'
if [[ "$SKIP_RUST" -eq 0 ]]; then
  command -v cargo >/dev/null 2>&1 || fail 'cargo is required unless --skip-rust is given'
  run_step 'Rust release binaries' bash -c "cd '$ROOT' && cargo build --release"
else
  printf '  %-26s%s\n' 'Rust release binaries' 'skipped'
fi
[[ -x "$SERVER_BIN" ]] || fail "Server binary missing after build: $SERVER_BIN"
[[ -x "$AGENT_BIN" ]] || fail "Agent binary missing after build: $AGENT_BIN"
if [[ -n "$RUST_BEFORE" && "$RUST_BEFORE" != "$(sha256sum "$SERVER_BIN" "$AGENT_BIN" | sha256sum | cut -c1-16)" ]]; then
  RUST_CHANGED=1
fi

if [[ "$SKIP_WEB" -eq 0 ]]; then
  command -v npm >/dev/null 2>&1 || fail 'npm is required unless --skip-web is given'
  [[ -d "$WEB_DIR/node_modules" ]] || fail "WebUI dependencies are not installed: $WEB_DIR/node_modules"
  if [[ "$SKIP_TESTS" -eq 0 ]]; then
    run_step 'WebUI lint/type/test' web_lint_typecheck_test
  else
    printf '  %-26s%s\n' 'WebUI lint/type/test' 'skipped'
  fi
  # Staged beside dist so the swap is a same-filesystem rename, not a copy.
  STAGING="$WEB_DIR/dist.staging"
  run_step 'WebUI production build' web_build_staging
  if diff -r "$DIST" "$STAGING" >/dev/null 2>&1; then
    rm -rf "$STAGING"; STAGING=""
    printf '  %-26s%s\n' 'WebUI content' 'unchanged'
  else
    rm -rf "$DIST"
    mv "$STAGING" "$DIST"
    STAGING=""
    printf '  %-26s%s\n' 'WebUI content' 'replaced'
  fi
else
  printf '  %-26s%s\n' 'WebUI production build' 'skipped'
fi
[[ -f "$DIST/index.html" ]] || fail "WebUI entry point missing: $DIST/index.html"
WEB_AFTER="$(dist_fingerprint)"
if [[ "$WEB_BEFORE" != "$WEB_AFTER" ]]; then
  WEB_CHANGED=1
  printf '  %-26s%s\n' 'WebUI fingerprint' "$WEB_BEFORE -> $WEB_AFTER"
fi

# --- decide ------------------------------------------------------------------

STALE=0
server_serves_current_assets || STALE=1

NEED_RESTART=0
REASON=""
if [[ "$FORCE" -eq 1 ]]; then
  NEED_RESTART=1; REASON="--force"
elif [[ "$RUST_CHANGED" -eq 1 ]]; then
  NEED_RESTART=1; REASON="Rust binaries rebuilt"
elif [[ "$WEB_CHANGED" -eq 1 ]]; then
  NEED_RESTART=1; REASON="WebUI rebuilt"
elif [[ "$STALE" -eq 1 ]]; then
  NEED_RESTART=1; REASON="live Server serves a stale WebUI build"
fi

echo
echo 'Deploy'
if [[ "$NEED_RESTART" -eq 0 ]]; then
  printf '  %-26s%s\n' 'Restart' 'not needed'
  printf '  %-26s%s\n' 'Reason' 'running services already match the build on disk'
  echo
  echo "Services left untouched. Server: $BASE_URL"
  exit 0
fi
printf '  %-26s%s\n' 'Restart' "$REASON"

if [[ -n "$SERVER_EXEC" && "$SERVER_EXEC" != "$SERVER_BIN" ]]; then
  printf 'warning: %s runs %s, not the binary this script rebuilt (%s)\n' \
    "$SERVER_UNIT" "$SERVER_EXEC" "$SERVER_BIN" >&2
fi

systemctl --user restart "$SERVER_UNIT" || fail "could not restart $SERVER_UNIT"
if ! wait_ready; then
  journalctl --user -u "$SERVER_UNIT" -n 80 --no-pager >&2 || true
  fail "/health/ready did not become ready within ${TIMEOUT}s"
fi
printf '  %-26s%s\n' 'Readiness' 'ready'

if [[ "$RESTART_AGENT" -eq 1 ]]; then
  systemctl --user restart "$AGENT_UNIT" || fail "could not restart $AGENT_UNIT"
  printf '  %-26s%s\n' 'Agent restart' 'ok'
fi

# --- verify ------------------------------------------------------------------

echo
echo 'Verify'
if ! server_serves_current_assets; then
  fail 'the restarted Server still does not serve the current WebUI build'
fi
printf '  %-26s%s\n' 'WebUI assets' 'resolved and byte-identical'
printf '  %-26s%s\n' 'Health' "$READY_BODY"
for unit in "$SERVER_UNIT" "$AGENT_UNIT"; do
  [[ "$unit" == "$SERVER_UNIT" || "$RESTART_AGENT" -eq 1 ]] || continue
  printf '  %-26s%s\n' "$unit" "$(systemctl --user is-active "$unit" 2>/dev/null || printf 'unknown')"
done
printf '  %-26s%s\n' 'WebUI fingerprint' "$(dist_fingerprint)"
printf '\ndeploy complete: %s\n' "$BASE_URL"
