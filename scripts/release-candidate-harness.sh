#!/usr/bin/env bash
# Reproducibly build and exercise packaged Agent/Server/WebUI artifacts.
set -euo pipefail

ROOT="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
unavailable() { printf 'Release-candidate harness: UNAVAILABLE (%s)\n' "$1"; exit 2; }
required_commands=(awk basename cat cargo chmod cp curl find grep head install jq ln mkdir mktemp node npm openssl python3 realpath rm sed seq sleep stat tail tar)
for command in "${required_commands[@]}"; do
  command -v "$command" >/dev/null 2>&1 || unavailable "missing required command: $command"
done
RUNS_ROOT="$ROOT/target/release-candidate-runs"
if [[ -n "${PLATPULSE_RC_RUNS_ROOT:-}" ]]; then RUNS_ROOT="$PLATPULSE_RC_RUNS_ROOT"; fi
RECOVERY_ROOT="${PLATPULSE_RC_RECOVERY_ROOT:-$ROOT/target/release-candidate-recovery}"
mkdir -p "$RUNS_ROOT" "$RECOVERY_ROOT"
RUN_ROOT="$(mktemp -d "$RUNS_ROOT/run.XXXXXX")"
RECOVERY_OUTPUT="$RECOVERY_ROOT/$(basename "$RUN_ROOT")"
PACKAGE_DIR="$RUN_ROOT/package"
ARCHIVE="$RUN_ROOT/platpulse-server.tar.gz"
AGENT_ARCHIVE=""
AGENT_EXTRACTED="$RUN_ROOT/agent-extracted"
EXTRACTED="$RUN_ROOT/extracted"
STATE_DIR="$RUN_ROOT/state"
BACKUP_DIR="$RUN_ROOT/backups"
CONFIG="$RUN_ROOT/server.toml"
SERVER_LOG="$RUN_ROOT/server.log"
CLI_LOG="$RUN_ROOT/cli.log"
DIAGNOSTICS="$RUN_ROOT/diagnostics.txt"
OWNER_COOKIE="$RUN_ROOT/owner.cookies"
VIEWER_COOKIE="$RUN_ROOT/viewer.cookies"
SSE_HEADERS="$RUN_ROOT/admin-events.headers"
SSE_OUTPUT="$RUN_ROOT/admin-events.sse"
REQUEST_IDS="$RUN_ROOT/request-ids.log"
: > "$REQUEST_IDS"
LAST_REQUEST_ID=unknown
FAILURE_REASON=unknown
SERVER_PID=""
SSE_PID=""
DEV_SERVER_PID=""
PROXY_SERVER_PID=""

# shellcheck disable=SC2329
cleanup() {
  local code=$?
  local server_status=not_started
  if [[ -n "$SSE_PID" ]] && kill -0 "$SSE_PID" 2>/dev/null; then kill "$SSE_PID" 2>/dev/null || true; fi
  if [[ -n "$SERVER_PID" ]]; then
    if kill -0 "$SERVER_PID" 2>/dev/null; then kill -TERM "$SERVER_PID" 2>/dev/null || true; fi
    if wait "$SERVER_PID" 2>/dev/null; then server_status=0; else server_status=$?; fi
  fi
  for aux_pid in "$DEV_SERVER_PID" "$PROXY_SERVER_PID"; do
    if [[ -n "$aux_pid" ]] && kill -0 "$aux_pid" 2>/dev/null; then
      kill -TERM "$aux_pid" 2>/dev/null || true
      wait "$aux_pid" 2>/dev/null || true
    fi
  done
  rm -f "$RUN_ROOT"/*.cookies "$RUN_ROOT"/*.headers "$RUN_ROOT"/*.json "$RUN_ROOT"/*.body "$RUN_ROOT"/*.sse "$RUN_ROOT"/*.status "$RUN_ROOT"/*.txt "$RUN_ROOT"/enrollment-output "$RUN_ROOT"/owner-password "$RUN_ROOT"/viewer-password "$RUN_ROOT"/tls-key.pem "$RUN_ROOT"/insecure-key.pem "$RUN_ROOT"/mismatch-key.pem "$STATE_DIR/server-pepper" "$STATE_DIR"/platpulse.db*
  if [[ "$code" -eq 0 ]]; then
    rm -rf "$RUN_ROOT"
    printf 'Release-candidate harness: PASS\n'
  elif [[ "$code" -eq 2 ]]; then
    rm -rf "$RUN_ROOT"
    printf 'Release-candidate harness: UNAVAILABLE\n'
  else
    {
      printf 'harness_exit_status=%s\n' "$code"
      printf 'failure_reason=%s\n' "$FAILURE_REASON"
      printf 'release_artifact=%s\n' "$ARCHIVE"
      printf 'extracted_artifact=%s\n' "$EXTRACTED"
      printf 'configuration=%s\n' "$CONFIG"
      printf 'server_pid=%s\n' "$SERVER_PID"
      printf 'server_exit_status=%s\n' "$server_status"
      printf 'last_request_id=%s\n' "$LAST_REQUEST_ID"
      printf 'request_ids:\n'
      if [[ -f "$REQUEST_IDS" ]]; then cat "$REQUEST_IDS"; fi
      printf 'preserved_test_artifacts=%s\n' "$RUN_ROOT"
    } > "$DIAGNOSTICS"
    printf 'Release-candidate harness: FAIL (artifacts preserved at %s)\n' "$RUN_ROOT" >&2
  fi
}
trap cleanup EXIT

fail() { FAILURE_REASON="$1"; printf 'Release-candidate harness: FAIL (%s)\n' "$1" >&2; exit 1; }

# Security assertions intentionally retain only sanitized response bodies on failure.
expect_security_status() {
  local name="$1" expected="$2"; shift 2
  local body="$RUN_ROOT/security-${name}.body" status
  status="$(curl -sS --connect-timeout 2 --max-time 10 "$@" -o "$body" -w '%{http_code}' 2>/dev/null || true)"
  [[ "$status" == "$expected" ]] || fail "security matrix ${name}: expected HTTP ${expected}, got ${status}"
}

expect_security_error() {
  local name="$1" expected_status="$2" expected_code="$3"; shift 3
  expect_security_status "$name" "$expected_status" "$@"
  jq -e --arg code "$expected_code" '.error.code == $code and (.error.message | type == "string") and (.error.message | test("pp_(agent|enroll)_|BEGIN .*PRIVATE KEY|/home/|/tmp/"; "i") | not)' "$RUN_ROOT/security-${name}.body" >/dev/null || fail "security matrix ${name}: error envelope was unstable or leaked sensitive material"
}

printf 'Release-candidate harness: building packaged artifact\n'
if ! PLATPULSE_RELEASE_ARCHIVE="$ARCHIVE" "$ROOT/scripts/package-release.sh" "$PACKAGE_DIR" >"$CLI_LOG" 2>&1; then fail 'release artifact build failed; see preserved CLI log'; fi
[[ -f "$ARCHIVE" ]] || fail 'release artifact was not produced'
"$ROOT/scripts/validate-release.sh" --archive "$ARCHIVE" --kind server || fail 'Server release archive failed policy validation'
AGENT_ARCHIVE="$(find "$PACKAGE_DIR/release-set" -maxdepth 1 -type f -name 'platpulse-agent-*.tar.gz' -print -quit)"
[[ -f "$AGENT_ARCHIVE" ]] || fail 'Agent release artifact was not produced'
"$ROOT/scripts/validate-release.sh" --archive "$AGENT_ARCHIVE" --kind agent || fail 'Agent release archive failed policy validation'
mkdir -p "$EXTRACTED" "$AGENT_EXTRACTED"
tar -xzf "$ARCHIVE" -C "$EXTRACTED" || fail 'release artifact could not be unpacked'
tar -xzf "$AGENT_ARCHIVE" -C "$AGENT_EXTRACTED" || fail 'Agent release artifact could not be unpacked'
AGENT="$AGENT_EXTRACTED/usr/bin/platpulse-agent"
"$AGENT" --help >/dev/null || fail 'packaged Agent --help failed'
NODE_ID="$($AGENT generate-node-id)"
[[ "$NODE_ID" =~ ^[0-9a-f-]{36}$ ]] || fail 'packaged Agent generate-node-id smoke failed'
SERVER="$EXTRACTED/usr/bin/platpulse-server"
WEB_ROOT="$EXTRACTED/usr/share/platpulse/web"
[[ -x "$SERVER" ]] || fail 'packaged Server binary is missing'
[[ -f "$WEB_ROOT/index.html" ]] || fail 'packaged WebUI index is missing'

PORT="$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
METRICS_PORT="$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
OWNER_PASSWORD="${PLATPULSE_RC_OWNER_PASSWORD:-rc-owner-password-2026}"
VIEWER_PASSWORD="${PLATPULSE_RC_VIEWER_PASSWORD:-rc-viewer-password-2026}"
OWNER_PASSWORD_FILE="$RUN_ROOT/owner-password"
VIEWER_PASSWORD_FILE="$RUN_ROOT/viewer-password"
printf '%s\n' "$OWNER_PASSWORD" > "$OWNER_PASSWORD_FILE"
printf '%s\n' "$VIEWER_PASSWORD" > "$VIEWER_PASSWORD_FILE"
chmod 600 "$OWNER_PASSWORD_FILE" "$VIEWER_PASSWORD_FILE"
BASE_URL="https://127.0.0.1:$PORT"
TLS_CERT="$RUN_ROOT/tls-cert.pem"
TLS_KEY="$RUN_ROOT/tls-key.pem"
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$TLS_KEY" -out "$TLS_CERT" -days 1 -subj '/CN=127.0.0.1' >/dev/null 2>&1
chmod 600 "$TLS_KEY"

printf 'Release-candidate harness: checking transport refusal and TLS validation\n'
PLAINTEXT_CONFIG="$RUN_ROOT/plaintext.toml"
cat > "$PLAINTEXT_CONFIG" <<EOF
state_dir = "$RUN_ROOT/plaintext-state"
listen = "0.0.0.0:8080"
EOF
if "$SERVER" serve --config "$PLAINTEXT_CONFIG" >"$RUN_ROOT/plaintext.log" 2>&1; then
  fail 'non-loopback plaintext production listener was accepted'
fi
grep -q 'refusing to bind' "$RUN_ROOT/plaintext.log" || fail 'plaintext refusal diagnostic was not stable'
INVALID_TLS_CONFIG="$RUN_ROOT/invalid-tls.toml"
cat > "$INVALID_TLS_CONFIG" <<EOF
state_dir = "$RUN_ROOT/invalid-tls-state"
public_base_url = "https://127.0.0.1:8443"
[tls]
cert_chain_file = "$TLS_CERT"
private_key_file = "$RUN_ROOT/missing-key.pem"
EOF
if "$SERVER" serve --config "$INVALID_TLS_CONFIG" >"$RUN_ROOT/invalid-tls.log" 2>&1; then
  fail 'missing native TLS private key was accepted'
fi
grep -q 'native TLS private-key material is invalid or insecure' "$RUN_ROOT/invalid-tls.log" || fail 'invalid TLS diagnostic was not stable or redacted'
INSECURE_TLS_CONFIG="$RUN_ROOT/insecure-tls.toml"
cp "$TLS_KEY" "$RUN_ROOT/insecure-key.pem"
chmod 640 "$RUN_ROOT/insecure-key.pem"
cat > "$INSECURE_TLS_CONFIG" <<EOF
state_dir = "$RUN_ROOT/insecure-tls-state"
public_base_url = "https://127.0.0.1:8443"
[tls]
cert_chain_file = "$TLS_CERT"
private_key_file = "$RUN_ROOT/insecure-key.pem"
EOF
if "$SERVER" serve --config "$INSECURE_TLS_CONFIG" >"$RUN_ROOT/insecure-tls.log" 2>&1; then
  fail 'insecure native TLS private key was accepted'
fi
grep -q 'native TLS private-key material is invalid or insecure' "$RUN_ROOT/insecure-tls.log" || fail 'insecure TLS diagnostic was not stable or redacted'
MALFORMED_TLS_CONFIG="$RUN_ROOT/malformed-tls.toml"
printf 'not a certificate\n' > "$RUN_ROOT/malformed-cert.pem"
cat > "$MALFORMED_TLS_CONFIG" <<EOF
state_dir = "$RUN_ROOT/malformed-tls-state"
public_base_url = "https://127.0.0.1:8443"
[tls]
cert_chain_file = "$RUN_ROOT/malformed-cert.pem"
private_key_file = "$TLS_KEY"
EOF
if "$SERVER" serve --config "$MALFORMED_TLS_CONFIG" >"$RUN_ROOT/malformed-tls.log" 2>&1; then
  fail 'malformed native TLS certificate was accepted'
fi
grep -q 'native TLS certificate and private-key material are incompatible' "$RUN_ROOT/malformed-tls.log" || fail 'malformed TLS diagnostic was not stable or redacted'
MISMATCHED_TLS_CONFIG="$RUN_ROOT/mismatched-tls.toml"
openssl genrsa -out "$RUN_ROOT/mismatch-key.pem" 2048 >/dev/null 2>&1
chmod 600 "$RUN_ROOT/mismatch-key.pem"
cat > "$MISMATCHED_TLS_CONFIG" <<EOF
state_dir = "$RUN_ROOT/mismatched-tls-state"
public_base_url = "https://127.0.0.1:8443"
[tls]
cert_chain_file = "$TLS_CERT"
private_key_file = "$RUN_ROOT/mismatch-key.pem"
EOF
if "$SERVER" serve --config "$MISMATCHED_TLS_CONFIG" >"$RUN_ROOT/mismatched-tls.log" 2>&1; then
  fail 'mismatched native TLS key was accepted'
fi
grep -q 'native TLS certificate and private-key material are incompatible' "$RUN_ROOT/mismatched-tls.log" || fail 'mismatched TLS diagnostic was not stable or redacted'
SYMLINKED_TLS_CONFIG="$RUN_ROOT/symlinked-tls.toml"
ln -s "$TLS_KEY" "$RUN_ROOT/symlink-key.pem"
cat > "$SYMLINKED_TLS_CONFIG" <<EOF
state_dir = "$RUN_ROOT/symlinked-tls-state"
public_base_url = "https://127.0.0.1:8443"
[tls]
cert_chain_file = "$TLS_CERT"
private_key_file = "$RUN_ROOT/symlink-key.pem"
EOF
if "$SERVER" serve --config "$SYMLINKED_TLS_CONFIG" >"$RUN_ROOT/symlinked-tls.log" 2>&1; then
  fail 'symlink-substituted native TLS key was accepted'
fi
grep -q 'native TLS private-key material is invalid or insecure' "$RUN_ROOT/symlinked-tls.log" || fail 'symlinked TLS diagnostic was not stable or redacted'

mkdir -p "$STATE_DIR"
cat > "$CONFIG" <<EOF
state_dir = "$STATE_DIR"
db_path = "$STATE_DIR/platpulse.db"
backup_dir = "$BACKUP_DIR"
pepper_file = "$STATE_DIR/server-pepper"
web_root = "$WEB_ROOT"
listen = "0.0.0.0:$PORT"
public_base_url = "$BASE_URL"
development = false
[tls]
cert_chain_file = "$TLS_CERT"
private_key_file = "$TLS_KEY"
[metrics]
enabled = true
listen = "127.0.0.1:$METRICS_PORT"
EOF

# The direct-TLS run uses the same external boundary as production. The
# harness's other HTTP checks must accept the intentionally self-signed test
# certificate without ever placing it in the release artifact.
curl() { command curl --insecure "$@"; }


printf 'Release-candidate harness: provisioning isolated identities and Network\n'
"$SERVER" init --config "$CONFIG" >>"$CLI_LOG" 2>&1 || fail 'isolated Server init failed; see preserved CLI log'
"$SERVER" owner create --config "$CONFIG" --username rc-owner <"$OWNER_PASSWORD_FILE" >>"$CLI_LOG" 2>&1 || fail 'Owner provisioning failed; see preserved CLI log'
"$SERVER" viewer create --config "$CONFIG" --username rc-viewer <"$VIEWER_PASSWORD_FILE" >>"$CLI_LOG" 2>&1 || fail 'Viewer provisioning failed; see preserved CLI log'
"$SERVER" network create --config "$CONFIG" --key platon-mainnet --display-name 'PlatON Mainnet' --genesis-hash "0x$(printf 'a%.0s' {1..64})" --chain-id 210425 --p2p-network-id 210425 --address-hrp lat >>"$CLI_LOG" 2>&1 || fail 'Network provisioning failed; see preserved CLI log'
"$SERVER" network create --config "$CONFIG" --key platon-testnet --display-name 'PlatON Testnet' --genesis-hash "0x$(printf 'b%.0s' {1..64})" --chain-id 2206131 --p2p-network-id 2206131 --address-hrp lat >>"$CLI_LOG" 2>&1 || fail 'Network provisioning failed; see preserved CLI log'

UNSAFE_METRICS_CONFIG="$RUN_ROOT/unsafe-metrics.toml"
cat > "$UNSAFE_METRICS_CONFIG" <<EOF
state_dir = "$STATE_DIR"
db_path = "$STATE_DIR/platpulse.db"
pepper_file = "$STATE_DIR/server-pepper"
web_root = "$WEB_ROOT"
listen = "127.0.0.1:$PORT"
public_base_url = "http://127.0.0.1:$PORT"
[metrics]
enabled = true
listen = "0.0.0.0:$METRICS_PORT"
EOF
if "$SERVER" serve --config "$UNSAFE_METRICS_CONFIG" >"$RUN_ROOT/unsafe-metrics.log" 2>&1; then
  fail 'non-loopback plaintext metrics listener was accepted'
fi
grep -q 'refusing to bind' "$RUN_ROOT/unsafe-metrics.log" || fail 'metrics transport refusal diagnostic was not stable'

printf 'Release-candidate harness: starting packaged Server\n'
"$SERVER" serve --config "$CONFIG" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
live_status=""
for _ in $(seq 1 100); do
  kill -0 "$SERVER_PID" 2>/dev/null || fail 'packaged Server exited before health check'
  live_status="$(curl -sS --connect-timeout 2 --max-time 5 -o "$RUN_ROOT/live.body" -w '%{http_code}' "$BASE_URL/health/live" 2>/dev/null || true)"
  [[ "$live_status" == 200 ]] && break
  sleep 0.1
done
kill -0 "$SERVER_PID" 2>/dev/null || fail 'packaged Server exited before health check'
[[ "$live_status" == 200 ]] || fail 'health/live did not become ready'

METRICS_BASE_URL="https://127.0.0.1:$METRICS_PORT"
printf 'Release-candidate harness: checking isolated Prometheus metrics\n'
metrics_status=""
for _ in $(seq 1 50); do
  metrics_status="$(curl -sS --connect-timeout 2 --max-time 5 -D "$RUN_ROOT/metrics.headers" -o "$RUN_ROOT/metrics.body" -w '%{http_code}' "$METRICS_BASE_URL/metrics" 2>/dev/null || true)"
  [[ "$metrics_status" == 200 ]] && break
  sleep 0.1
done
[[ "$metrics_status" == 200 ]] || fail 'internal metrics listener did not become ready'
grep -qi 'text/plain; version=0.0.4' "$RUN_ROOT/metrics.headers" || fail 'metrics content type was not Prometheus-compatible'
grep -q '^# TYPE platpulse_http_requests_total counter$' "$RUN_ROOT/metrics.body" || fail 'metrics exposition omitted request family type'
[[ "$(grep -c '^# HELP platpulse_http_requests_total ' "$RUN_ROOT/metrics.body")" == 1 ]] || fail 'metrics repeated HELP declarations for one family'
grep -q 'platpulse_readiness{component="critical_workers"} 1' "$RUN_ROOT/metrics.body" || fail 'metrics omitted healthy critical-worker readiness'
grep -q '^platpulse_liveness 1$' "$RUN_ROOT/metrics.body" || fail 'metrics omitted liveness state'
# Critical workers heartbeat on their own cadence. Capture readiness immediately
# after metrics so the smoke does not mistake a later honest stale response for
# a startup failure.
ready_status="$(curl -sS --connect-timeout 2 --max-time 5 -D "$RUN_ROOT/ready.headers" -o "$RUN_ROOT/ready.body" -w '%{http_code}' "$BASE_URL/health/ready" 2>/dev/null || true)"
[[ "$ready_status" == 200 ]] || fail "health/ready did not become ready (status $ready_status)"
for uri in /api/anything /health/live /; do
  route_status="$(curl -sS --connect-timeout 2 --max-time 5 -o /dev/null -w '%{http_code}' "$METRICS_BASE_URL$uri" 2>/dev/null || true)"
  [[ "$route_status" == 404 ]] || fail "metrics listener exposed non-metrics route: $uri"
done

printf 'Release-candidate harness: checking development and trusted-proxy modes\n'
DEV_PORT="$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
DEV_CONFIG="$RUN_ROOT/development.toml"
cat > "$DEV_CONFIG" <<EOF
state_dir = "$RUN_ROOT/development-state"
listen = "127.0.0.1:$DEV_PORT"
public_base_url = "http://127.0.0.1:$DEV_PORT"
development = true
EOF
"$SERVER" init --config "$DEV_CONFIG" >>"$CLI_LOG" 2>&1 || fail 'development-mode init failed'
"$SERVER" serve --config "$DEV_CONFIG" >"$RUN_ROOT/development.log" 2>&1 &
DEV_SERVER_PID=$!
for _ in $(seq 1 50); do
  kill -0 "$DEV_SERVER_PID" 2>/dev/null || fail 'development-mode Server exited before health check'
  dev_status="$(curl -sS --connect-timeout 2 --max-time 5 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DEV_PORT/health/live" 2>/dev/null || true)"
  [[ "$dev_status" == 200 ]] && break
  sleep 0.1
done
[[ "${dev_status:-}" == 200 ]] || fail 'development-mode HTTP did not become ready'
kill -TERM "$DEV_SERVER_PID" 2>/dev/null || true
wait "$DEV_SERVER_PID" 2>/dev/null || true
DEV_SERVER_PID=""

PROXY_PORT="$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
PROXY_CONFIG="$RUN_ROOT/trusted-proxy.toml"
cat > "$PROXY_CONFIG" <<EOF
state_dir = "$RUN_ROOT/trusted-proxy-state"
listen = "0.0.0.0:$PROXY_PORT"
public_base_url = "https://127.0.0.1:$PROXY_PORT"
trusted_proxy_cidrs = ["127.0.0.1/32"]
trusted_proxy_scheme = "https"
EOF
"$SERVER" init --config "$PROXY_CONFIG" >>"$CLI_LOG" 2>&1 || fail 'trusted-proxy init failed'
"$SERVER" serve --config "$PROXY_CONFIG" >"$RUN_ROOT/trusted-proxy.log" 2>&1 &
PROXY_SERVER_PID=$!
for _ in $(seq 1 50); do
  kill -0 "$PROXY_SERVER_PID" 2>/dev/null || fail 'trusted-proxy Server exited before health check'
  proxy_status="$(curl -sS --connect-timeout 2 --max-time 5 -D "$RUN_ROOT/proxy.headers" -H 'Forwarded: proto=https' -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PROXY_PORT/health/live" 2>/dev/null || true)"
  [[ "$proxy_status" == 200 ]] && break
  sleep 0.1
done
[[ "${proxy_status:-}" == 200 ]] || fail 'trusted-proxy HTTPS assertion was not accepted'
grep -qi '^strict-transport-security: max-age=31536000; includeSubDomains' "$RUN_ROOT/proxy.headers" || fail 'trusted-proxy response did not include HSTS'
proxy_spoofed_status="$(curl -sS --connect-timeout 2 --max-time 5 -H 'Forwarded: proto=http' -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PROXY_PORT/health/live" 2>/dev/null || true)"
[[ "$proxy_spoofed_status" == 403 ]] || fail 'trusted-proxy HTTP scheme spoof was not rejected'
kill -TERM "$PROXY_SERVER_PID" 2>/dev/null || true
wait "$PROXY_SERVER_PID" 2>/dev/null || true
PROXY_SERVER_PID=""

request_id() {
  local id
  id="$(awk -F': ' 'tolower($1) == "x-request-id" { gsub("\\r", "", $2); print $2; exit }' "$1")"
  if [[ -n "$id" ]]; then printf '%s %s\n' "$(basename "$1")" "$id" >> "$REQUEST_IDS"; fi
  printf '%s' "$id"
}
request() {
  local name="$1"; shift
  local headers="$RUN_ROOT/$name.headers" body="$RUN_ROOT/$name.body" status="$RUN_ROOT/$name.status"
  curl -sS --connect-timeout 2 --max-time 10 -D "$headers" -o "$body" -w '%{http_code}' "$@" >"$status" || fail "HTTP request failed: $name"
  LAST_REQUEST_ID="$(request_id "$headers")"; LAST_REQUEST_ID="${LAST_REQUEST_ID:-unknown}"
  [[ "$(cat "$status")" == 200 ]] || fail "HTTP request $name returned $(cat "$status") (request $LAST_REQUEST_ID)"
}

printf 'Release-candidate harness: checking health and human boundaries\n'
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/ready.headers")"; LAST_REQUEST_ID="${LAST_REQUEST_ID:-unknown}"
jq -e '.status == "ready"' "$RUN_ROOT/ready.body" >/dev/null || fail "health/ready was not ready (request $LAST_REQUEST_ID)"
grep -qi '^strict-transport-security: max-age=31536000; includeSubDomains' "$RUN_ROOT/ready.headers" || fail 'native TLS response did not include HSTS'
request web-index "$BASE_URL/"
grep -q '<div id="root"' "$RUN_ROOT/web-index.body" || fail "packaged WebUI index was not served (request $LAST_REQUEST_ID)"
WEB_ASSET_PATH="$(grep -oE '/assets/[^" ]+' "$RUN_ROOT/web-index.body" | head -n 1)"
[[ "$WEB_ASSET_PATH" == /assets/* ]] || fail "packaged WebUI asset reference was missing (request $LAST_REQUEST_ID)"
request web-asset "$BASE_URL$WEB_ASSET_PATH"
cat > "$RUN_ROOT/owner-login.json" <<EOF
{"username":"rc-owner","password":$(jq -Rn --arg value "$OWNER_PASSWORD" '$value')}
EOF
curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/owner-login.headers" -c "$OWNER_COOKIE" -o "$RUN_ROOT/owner-login.body" -H 'Content-Type: application/json' -H "Origin: $BASE_URL" --data-binary "@$RUN_ROOT/owner-login.json" "$BASE_URL/api/public/v1/login" -w '%{http_code}' >"$RUN_ROOT/owner-login.status" || fail 'Owner login request failed'
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/owner-login.headers")"; [[ "$(cat "$RUN_ROOT/owner-login.status")" == 200 ]] || fail "Owner login failed (request $LAST_REQUEST_ID)"
jq -e '.session.role == "owner" and (.csrfToken | length > 0)' "$RUN_ROOT/owner-login.body" >/dev/null || fail "Owner login response was invalid (request $LAST_REQUEST_ID)"
cat > "$RUN_ROOT/viewer-login.json" <<EOF
{"username":"rc-viewer","password":$(jq -Rn --arg value "$VIEWER_PASSWORD" '$value')}
EOF
curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/viewer-login.headers" -c "$VIEWER_COOKIE" -o "$RUN_ROOT/viewer-login.body" -H 'Content-Type: application/json' -H "Origin: $BASE_URL" --data-binary "@$RUN_ROOT/viewer-login.json" "$BASE_URL/api/public/v1/login" -w '%{http_code}' >"$RUN_ROOT/viewer-login.status" || fail 'Viewer login request failed'
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/viewer-login.headers")"; [[ "$(cat "$RUN_ROOT/viewer-login.status")" == 200 ]] || fail "Viewer login failed (request $LAST_REQUEST_ID)"
jq -e '.session.role == "viewer"' "$RUN_ROOT/viewer-login.body" >/dev/null || fail "Viewer login response was invalid (request $LAST_REQUEST_ID)"
OWNER_CSRF="$(jq -r '.csrfToken' "$RUN_ROOT/owner-login.body")"
VIEWER_CSRF="$(jq -r '.csrfToken' "$RUN_ROOT/viewer-login.body")"

printf 'Release-candidate harness: running packaged security matrix\n'
grep -qi '^set-cookie: __Host-platpulse_session=' "$RUN_ROOT/owner-login.headers" || fail 'production session cookie was not host-prefixed'
grep -qi '^set-cookie: __Host-platpulse_session=.*Secure' "$RUN_ROOT/owner-login.headers" || fail 'session cookie was not Secure'
grep -qi '^set-cookie: __Host-platpulse_session=.*HttpOnly' "$RUN_ROOT/owner-login.headers" || fail 'session cookie was not HttpOnly'
grep -qi '^set-cookie: __Host-platpulse_session=.*SameSite=Lax' "$RUN_ROOT/owner-login.headers" || fail 'session cookie did not use SameSite=Lax'
grep -qi '^cache-control: no-store' "$RUN_ROOT/owner-login.headers" || fail 'login response was cacheable'
expect_security_error guest-public 401 auth_required "$BASE_URL/api/public/v1/networks"
expect_security_error guest-admin 401 auth_required "$BASE_URL/api/admin/v1/access"
expect_security_error viewer-admin 403 owner_required -b "$VIEWER_COOKIE" "$BASE_URL/api/admin/v1/access"
expect_security_error viewer-agent 401 agent_auth_required -b "$VIEWER_COOKIE" "$BASE_URL/api/agent/v1/time"
expect_security_status owner-admin 200 -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/access"
session_status="$(curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/security-session.headers" -b "$OWNER_COOKIE" -o "$RUN_ROOT/security-session.body" -w '%{http_code}' "$BASE_URL/api/public/v1/session" 2>/dev/null || true)"
[[ "$session_status" == 200 ]] || fail 'packaged session probe failed'
grep -qi '^cache-control: no-store' "$RUN_ROOT/security-session.headers" || fail 'session response was cacheable'
expect_security_error wrong-origin-login 403 origin_validation_failed -H 'Content-Type: application/json' -H 'Origin: https://evil.example' --data-binary "@$RUN_ROOT/owner-login.json" "$BASE_URL/api/public/v1/login"
expect_security_error missing-origin-login 403 origin_validation_failed -H 'Content-Type: application/json' --data-binary "@$RUN_ROOT/owner-login.json" "$BASE_URL/api/public/v1/login"
printf '{"guestEnabled":false}' > "$RUN_ROOT/security-valid-access.json"
expect_security_error missing-csrf 403 csrf_validation_failed -b "$OWNER_COOKIE" -H 'Content-Type: application/json' --data-binary "@$RUN_ROOT/security-valid-access.json" -X PUT "$BASE_URL/api/admin/v1/access"
printf '{not-json' > "$RUN_ROOT/security-malformed.json"
expect_security_error malformed-human-json 400 invalid_json -b "$OWNER_COOKIE" -H 'Content-Type: application/json' -H "Origin: $BASE_URL" -H "X-CSRF-Token: $OWNER_CSRF" --data-binary "@$RUN_ROOT/security-malformed.json" -X PUT "$BASE_URL/api/admin/v1/access"
expect_security_status api-not-found 404 "$BASE_URL/api/not-found"
expect_security_status encoded-traversal 404 --path-as-is "$BASE_URL/assets/%2e%2e/%2e%2e/server.toml"
expect_security_status external-redirect-probe 200 "$BASE_URL/login?next=https%3A%2F%2Fevil.example"
! grep -qi '^location: https://evil.example' "$RUN_ROOT/security-external-redirect-probe.body" || fail 'external redirect probe was accepted'
VIEWER_OLD_COOKIE="$RUN_ROOT/viewer-old.cookies"
VIEWER_ROTATED_COOKIE="$RUN_ROOT/viewer-rotated.cookies"
cp "$VIEWER_COOKIE" "$VIEWER_OLD_COOKIE"
old_session="$(awk '$6 == "__Host-platpulse_session" { print $7; exit }' "$VIEWER_OLD_COOKIE")"
curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/viewer-rotate.headers" -c "$VIEWER_ROTATED_COOKIE" -o "$RUN_ROOT/viewer-rotate.body" -b "$VIEWER_OLD_COOKIE" -H 'Content-Type: application/json' -H "Origin: $BASE_URL" --data-binary "@$RUN_ROOT/viewer-login.json" "$BASE_URL/api/public/v1/login" -w '%{http_code}' >"$RUN_ROOT/viewer-rotate.status" || fail 'session rotation request failed'
[[ "$(cat "$RUN_ROOT/viewer-rotate.status")" == 200 ]] || fail 'session rotation login was rejected'
new_session="$(awk '$6 == "__Host-platpulse_session" { print $7; exit }' "$VIEWER_ROTATED_COOKIE")"
[[ -n "$old_session" && -n "$new_session" && "$old_session" != "$new_session" ]] || fail 'successful login did not rotate the production Session ID'
expect_security_error rotated-old-session 401 auth_required -b "$VIEWER_OLD_COOKIE" "$BASE_URL/api/public/v1/session"
mv "$VIEWER_ROTATED_COOKIE" "$VIEWER_COOKIE"
VIEWER_CSRF="$(jq -r '.csrfToken' "$RUN_ROOT/viewer-rotate.body")"
request public-networks -b "$VIEWER_COOKIE" "$BASE_URL/api/public/v1/networks"
jq -e 'type == "array"' "$RUN_ROOT/public-networks.body" >/dev/null || fail "Viewer REST projection was invalid (request $LAST_REQUEST_ID)"
LOGOUT_COOKIE="$RUN_ROOT/logout.cookies"
cp "$VIEWER_COOKIE" "$LOGOUT_COOKIE"
logout_status="$(curl -sS --connect-timeout 2 --max-time 10 -b "$LOGOUT_COOKIE" -H "Origin: $BASE_URL" -H "X-CSRF-Token: $VIEWER_CSRF" -o "$RUN_ROOT/logout.body" -w '%{http_code}' -X POST "$BASE_URL/api/public/v1/logout" 2>/dev/null || true)"
[[ "$logout_status" == 204 ]] || fail 'logout did not revoke the packaged Session'
expect_security_error revoked-session 401 auth_required -b "$LOGOUT_COOKIE" "$BASE_URL/api/public/v1/session"

printf 'Release-candidate harness: opening authorized Admin SSE\n'
curl -sS --connect-timeout 2 -N -D "$SSE_HEADERS" -o "$SSE_OUTPUT" -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/events" 2>"$RUN_ROOT/sse.stderr" &
SSE_PID=$!
for _ in $(seq 1 50); do
  grep -q 'HTTP/.* 200' "$SSE_HEADERS" 2>/dev/null && break
  kill -0 "$SSE_PID" 2>/dev/null || fail 'authorized Admin SSE exited before connecting'
  sleep 0.1
done
grep -q 'HTTP/.* 200' "$SSE_HEADERS" || fail 'authorized Admin SSE did not return HTTP 200'
SSE_REQUEST_ID="$(request_id "$SSE_HEADERS")"
if [[ -n "$SSE_REQUEST_ID" ]]; then LAST_REQUEST_ID="$SSE_REQUEST_ID"; fi

printf 'Release-candidate harness: enrolling Agent and submitting two-Node report\n'
"$SERVER" agent create-enrollment-token --config "$CONFIG" >"$RUN_ROOT/enrollment-output" 2>>"$CLI_LOG" || fail 'Enrollment token provisioning failed; see preserved CLI log'
ENROLLMENT_TOKEN="$(tail -n 1 "$RUN_ROOT/enrollment-output")"
[[ "$ENROLLMENT_TOKEN" == pp_enroll_* ]] || fail 'Enrollment token output was invalid'
curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/enroll.headers" -o "$RUN_ROOT/enroll.body" -H "Authorization: Bearer $ENROLLMENT_TOKEN" -w '%{http_code}' -X POST "$BASE_URL/api/agent/v1/enroll" >"$RUN_ROOT/enroll.status" || fail 'Agent enrollment request failed'
expect_security_error enrollment-token-reuse 409 enrollment_token_consumed -H "Authorization: Bearer $ENROLLMENT_TOKEN" -X POST "$BASE_URL/api/agent/v1/enroll"
unset ENROLLMENT_TOKEN
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/enroll.headers")"; [[ "$(cat "$RUN_ROOT/enroll.status")" == 200 ]] || fail "Agent enrollment failed (request $LAST_REQUEST_ID)"
AGENT_ID="$(jq -r '.agent_id' "$RUN_ROOT/enroll.body")"
AGENT_EPOCH="$(jq -r '.agent_epoch' "$RUN_ROOT/enroll.body")"
AGENT_CREDENTIAL="$(jq -r '.credential' "$RUN_ROOT/enroll.body")"
[[ "$AGENT_ID" != null && -n "$AGENT_ID" ]] || fail "Enrollment did not return an Agent identity (request $LAST_REQUEST_ID)"
[[ "$AGENT_CREDENTIAL" == pp_agent_* ]] || fail "Enrollment did not return an Agent Credential (request $LAST_REQUEST_ID)"
expect_security_error agent-credential-public 401 auth_required -H "Authorization: Bearer $AGENT_CREDENTIAL" "$BASE_URL/api/public/v1/networks"
expect_security_error agent-credential-admin 401 auth_required -H "Authorization: Bearer $AGENT_CREDENTIAL" "$BASE_URL/api/admin/v1/access"
REPORT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
jq --arg agent_id "$AGENT_ID" --arg epoch "$AGENT_EPOCH" --arg report_id "$REPORT_ID" --arg boot_id "$BOOT_ID" '.agent_id = $agent_id | .agent_epoch = ($epoch | tonumber) | .report_id = $report_id | .boot_id = $boot_id | .report_sequence = 1' "$ROOT/crates/platpulse-core/tests/fixtures/report_v1_canonical.json" > "$RUN_ROOT/report.json"
jq -e '(.inventory.nodes | length >= 2) and ([.inventory.nodes[].node_id] | unique | length >= 2)' "$RUN_ROOT/report.json" >/dev/null || fail 'smoke report fixture does not contain two independent Nodes'
curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/report.headers" -o "$RUN_ROOT/report.body" -H "Authorization: Bearer $AGENT_CREDENTIAL" -H 'Content-Type: application/json' --data-binary "@$RUN_ROOT/report.json" -w '%{http_code}' "$BASE_URL/api/agent/v1/reports" >"$RUN_ROOT/report.status" || fail 'AgentReport request failed'
unset AGENT_CREDENTIAL
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/report.headers")"; [[ "$(cat "$RUN_ROOT/report.status")" == 200 ]] || fail "AgentReport failed (request $LAST_REQUEST_ID)"
jq -e --arg report_id "$REPORT_ID" '.receipt.report_id == $report_id and (.receipt.disposition == "accepted" or .receipt.disposition == "partially_accepted") and (.receipt.nodes | length) >= 2 and ([.receipt.nodes[].node_id] | unique | length >= 2) and all(.receipt.nodes[]; .current == "accepted") and any(.receipt.samples[]; .disposition == "accepted")' "$RUN_ROOT/report.body" >/dev/null || fail "Report Receipt did not commit two accepted Nodes (request $LAST_REQUEST_ID)"

printf 'Release-candidate harness: verifying projection and invalidation\n'
request admin-networks -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/networks"
jq -e 'length == 2 and any(.[]; .network_key == "platon-mainnet" and .genesis_hash == "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" and .chain_id == 210425) and any(.[]; .network_key == "platon-testnet" and .genesis_hash == "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" and .chain_id == 2206131)' "$RUN_ROOT/admin-networks.body" >/dev/null || fail "Admin REST projection did not contain the provisioned Networks (request $LAST_REQUEST_ID)"
request admin-agents -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/agents"
jq -e --arg agent_id "$AGENT_ID" 'any(.[]; .agent_id == $agent_id and .last_report_sequence == 1 and (.nodes | length) >= 2)' "$RUN_ROOT/admin-agents.body" >/dev/null || fail "Admin projection did not contain the committed two-Node report (request $LAST_REQUEST_ID)"
request admin-node-a -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/nodes/0195f2a1-0004-4004-8004-000000000004"
jq -e '.node_id == "0195f2a1-0004-4004-8004-000000000004" and .current_head == 100042 and .identity.state == "matched"' "$RUN_ROOT/admin-node-a.body" >/dev/null || fail "Admin Node current projection was not committed (request $LAST_REQUEST_ID)"
for _ in $(seq 1 50); do
  if grep -q 'event: invalidation' "$SSE_OUTPUT" && grep -q '"resource":"node"' "$SSE_OUTPUT"; then break; fi
  sleep 0.1
done
if ! grep -q 'event: invalidation' "$SSE_OUTPUT" || ! grep -q '"resource":"node"' "$SSE_OUTPUT"; then fail "authorized Admin SSE did not observe the Node invalidation (request $LAST_REQUEST_ID)"; fi

printf 'Release-candidate harness: creating sanitized scheduled backup\n'
"$SERVER" backup --config "$CONFIG" >"$RUN_ROOT/backup-output" 2>>"$CLI_LOG" || fail 'packaged Server backup command failed'
BACKUP_FILE="$(sed -n "s/^Created sanitized backup '\(.*\)'.$/\1/p" "$RUN_ROOT/backup-output")"
[[ "$BACKUP_FILE" == platpulse-*.db ]] || fail 'backup command did not report a safe artifact name'
[[ -f "$BACKUP_DIR/$BACKUP_FILE" ]] || fail 'backup command did not create the reported artifact'
[[ "$(stat -c '%a' "$BACKUP_DIR/$BACKUP_FILE")" == 600 ]] || fail 'backup artifact mode was not 0600'
! find "$BACKUP_DIR" -type f -name '*.part' -print -quit | grep -q . || fail 'backup left a partial artifact'

curl -sS --connect-timeout 2 --max-time 5 -o "$RUN_ROOT/metrics-final.body" "$METRICS_BASE_URL/metrics" || fail 'final metrics scrape failed'
! grep -Fq "$REPORT_ID" "$RUN_ROOT/metrics-final.body" || fail 'metrics exposed a raw report ID'
! grep -Fq "$AGENT_ID" "$RUN_ROOT/metrics-final.body" || fail 'metrics exposed a raw Agent ID'
for forbidden in node_id peer_id user_id agent_id ip_address credential password request_id report_id; do
  ! grep -Fqi "$forbidden" "$RUN_ROOT/metrics-final.body" || fail "metrics exposed forbidden field name: $forbidden"
done
! grep -Eq '([0-9]{1,3}\.){3}[0-9]{1,3}' "$RUN_ROOT/metrics-final.body" || fail 'metrics exposed an IP address'

python3 "$ROOT/scripts/release-recovery-rehearsal.py" --skip-package --server "$SERVER" --output "$RECOVERY_OUTPUT" || fail "packaged migration and recovery rehearsal failed"
printf 'Release-candidate harness: PASS (artifact=%s, request_id=%s, recovery_evidence=%s)\n' "$ARCHIVE" "$LAST_REQUEST_ID" "$RECOVERY_OUTPUT"
exit 0
