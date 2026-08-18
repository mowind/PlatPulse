#!/usr/bin/env bash
# Reproducibly build and exercise the packaged Server/WebUI artifact.
set -euo pipefail

ROOT="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
unavailable() { printf 'Release-candidate harness: UNAVAILABLE (%s)\n' "$1"; exit 2; }
required_commands=(awk basename cat cargo chmod cp curl find grep head install jq mkdir mktemp node npm python3 realpath rm sed seq sleep tail tar)
for command in "${required_commands[@]}"; do
  command -v "$command" >/dev/null 2>&1 || unavailable "missing required command: $command"
done
RUNS_ROOT="$ROOT/target/release-candidate-runs"
if [[ -n "${PLATPULSE_RC_RUNS_ROOT:-}" ]]; then RUNS_ROOT="$PLATPULSE_RC_RUNS_ROOT"; fi
mkdir -p "$RUNS_ROOT"
RUN_ROOT="$(mktemp -d "$RUNS_ROOT/run.XXXXXX")"
PACKAGE_DIR="$RUN_ROOT/package"
ARCHIVE="$RUN_ROOT/platpulse-server.tar.gz"
EXTRACTED="$RUN_ROOT/extracted"
STATE_DIR="$RUN_ROOT/state"
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

cleanup() {
  local code=$?
  local server_status=not_started
  if [[ -n "$SSE_PID" ]] && kill -0 "$SSE_PID" 2>/dev/null; then kill "$SSE_PID" 2>/dev/null || true; fi
  if [[ -n "$SERVER_PID" ]]; then
    if kill -0 "$SERVER_PID" 2>/dev/null; then kill -TERM "$SERVER_PID" 2>/dev/null || true; fi
    if wait "$SERVER_PID" 2>/dev/null; then server_status=0; else server_status=$?; fi
  fi
  rm -f "$RUN_ROOT"/*.cookies "$RUN_ROOT"/*.headers "$RUN_ROOT"/*.json "$RUN_ROOT"/*.body "$RUN_ROOT"/*.status "$RUN_ROOT"/*.txt "$RUN_ROOT"/enrollment-output "$RUN_ROOT"/owner-password "$RUN_ROOT"/viewer-password "$STATE_DIR/server-pepper" "$STATE_DIR"/platpulse.db*
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

printf 'Release-candidate harness: building packaged artifact\n'
if ! PLATPULSE_RELEASE_ARCHIVE="$ARCHIVE" "$ROOT/scripts/package-release.sh" "$PACKAGE_DIR" >"$CLI_LOG" 2>&1; then fail 'release artifact build failed; see preserved CLI log'; fi
[[ -f "$ARCHIVE" ]] || fail 'release artifact was not produced'
mkdir -p "$EXTRACTED"
tar -xzf "$ARCHIVE" -C "$EXTRACTED" || fail 'release artifact could not be unpacked'
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
OWNER_PASSWORD="${PLATPULSE_RC_OWNER_PASSWORD:-rc-owner-password-2026}"
VIEWER_PASSWORD="${PLATPULSE_RC_VIEWER_PASSWORD:-rc-viewer-password-2026}"
OWNER_PASSWORD_FILE="$RUN_ROOT/owner-password"
VIEWER_PASSWORD_FILE="$RUN_ROOT/viewer-password"
printf '%s\n' "$OWNER_PASSWORD" > "$OWNER_PASSWORD_FILE"
printf '%s\n' "$VIEWER_PASSWORD" > "$VIEWER_PASSWORD_FILE"
chmod 600 "$OWNER_PASSWORD_FILE" "$VIEWER_PASSWORD_FILE"
BASE_URL="http://127.0.0.1:$PORT"
mkdir -p "$STATE_DIR"
cat > "$CONFIG" <<EOF
state_dir = "$STATE_DIR"
db_path = "$STATE_DIR/platpulse.db"
pepper_file = "$STATE_DIR/server-pepper"
web_root = "$WEB_ROOT"
listen = "127.0.0.1:$PORT"
public_base_url = "$BASE_URL"
development = true
EOF

printf 'Release-candidate harness: provisioning isolated identities and Network\n'
"$SERVER" init --config "$CONFIG" >>"$CLI_LOG" 2>&1 || fail 'isolated Server init failed; see preserved CLI log'
"$SERVER" owner create --config "$CONFIG" --username rc-owner <"$OWNER_PASSWORD_FILE" >>"$CLI_LOG" 2>&1 || fail 'Owner provisioning failed; see preserved CLI log'
"$SERVER" viewer create --config "$CONFIG" --username rc-viewer <"$VIEWER_PASSWORD_FILE" >>"$CLI_LOG" 2>&1 || fail 'Viewer provisioning failed; see preserved CLI log'
"$SERVER" network create --config "$CONFIG" --key platon-mainnet --display-name 'PlatON Mainnet' --genesis-hash "0x$(printf 'a%.0s' {1..64})" --chain-id 210425 --p2p-network-id 210425 --address-hrp lat >>"$CLI_LOG" 2>&1 || fail 'Network provisioning failed; see preserved CLI log'
"$SERVER" network create --config "$CONFIG" --key platon-testnet --display-name 'PlatON Testnet' --genesis-hash "0x$(printf 'b%.0s' {1..64})" --chain-id 2206131 --p2p-network-id 2206131 --address-hrp lat >>"$CLI_LOG" 2>&1 || fail 'Network provisioning failed; see preserved CLI log'

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
request ready "$BASE_URL/health/ready"
jq -e '.status == "ready"' "$RUN_ROOT/ready.body" >/dev/null || fail "health/ready was not ready (request $LAST_REQUEST_ID)"
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
request public-networks -b "$VIEWER_COOKIE" "$BASE_URL/api/public/v1/networks"
jq -e 'type == "array"' "$RUN_ROOT/public-networks.body" >/dev/null || fail "Viewer REST projection was invalid (request $LAST_REQUEST_ID)"

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
unset ENROLLMENT_TOKEN
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/enroll.headers")"; [[ "$(cat "$RUN_ROOT/enroll.status")" == 200 ]] || fail "Agent enrollment failed (request $LAST_REQUEST_ID)"
AGENT_ID="$(jq -r '.agent_id' "$RUN_ROOT/enroll.body")"
AGENT_EPOCH="$(jq -r '.agent_epoch' "$RUN_ROOT/enroll.body")"
AGENT_CREDENTIAL="$(jq -r '.credential' "$RUN_ROOT/enroll.body")"
[[ "$AGENT_ID" != null && -n "$AGENT_ID" ]] || fail "Enrollment did not return an Agent identity (request $LAST_REQUEST_ID)"
[[ "$AGENT_CREDENTIAL" == pp_agent_* ]] || fail "Enrollment did not return an Agent Credential (request $LAST_REQUEST_ID)"
REPORT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
jq --arg agent_id "$AGENT_ID" --arg epoch "$AGENT_EPOCH" --arg report_id "$REPORT_ID" --arg boot_id "$BOOT_ID" '.agent_id = $agent_id | .agent_epoch = ($epoch | tonumber) | .report_id = $report_id | .boot_id = $boot_id | .report_sequence = 1' "$ROOT/crates/platpulse-core/tests/fixtures/report_v1_canonical.json" > "$RUN_ROOT/report.json"
jq -e '.inventory.nodes | length >= 2' "$RUN_ROOT/report.json" >/dev/null || fail 'smoke report fixture does not contain two independent Nodes'
curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/report.headers" -o "$RUN_ROOT/report.body" -H "Authorization: Bearer $AGENT_CREDENTIAL" -H 'Content-Type: application/json' --data-binary "@$RUN_ROOT/report.json" -w '%{http_code}' "$BASE_URL/api/agent/v1/reports" >"$RUN_ROOT/report.status" || fail 'AgentReport request failed'
unset AGENT_CREDENTIAL
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/report.headers")"; [[ "$(cat "$RUN_ROOT/report.status")" == 200 ]] || fail "AgentReport failed (request $LAST_REQUEST_ID)"
jq -e --arg report_id "$REPORT_ID" '.receipt.report_id == $report_id and (.receipt.disposition == "accepted" or .receipt.disposition == "partially_accepted") and (.receipt.nodes | length) >= 2 and all(.receipt.nodes[]; .current == "accepted") and any(.receipt.samples[]; .disposition == "accepted")' "$RUN_ROOT/report.body" >/dev/null || fail "Report Receipt did not commit two accepted Nodes (request $LAST_REQUEST_ID)"

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

printf 'Release-candidate harness: PASS (artifact=%s, request_id=%s)\n' "$ARCHIVE" "$LAST_REQUEST_ID"
exit 0
