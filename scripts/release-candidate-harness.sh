#!/usr/bin/env bash
# Reproducibly build and exercise the packaged Server/WebUI artifact.
set -euo pipefail

ROOT="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
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
UNAVAILABLE_REASON=unknown
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
    printf 'Release-candidate harness: PASS\\n'
  elif [[ "$code" -eq 2 ]]; then
    rm -rf "$RUN_ROOT"
    printf 'Release-candidate harness: UNAVAILABLE (%s)\\n' "$UNAVAILABLE_REASON"
  else
    {
      printf 'harness_exit_status=%s\\n' "$code"
      printf 'failure_reason=%s\\n' "$FAILURE_REASON"
      printf 'release_artifact=%s\\n' "$ARCHIVE"
      printf 'extracted_artifact=%s\\n' "$EXTRACTED"
      printf 'configuration=%s\\n' "$CONFIG"
      printf 'server_pid=%s\\n' "$SERVER_PID"
      printf 'server_exit_status=%s\\n' "$server_status"
      printf 'last_request_id=%s\\n' "$LAST_REQUEST_ID"
      printf 'request_ids:\\n'
      cat "$REQUEST_IDS" 2>/dev/null || true
      printf 'preserved_test_artifacts=%s\\n' "$RUN_ROOT"
    } > "$DIAGNOSTICS"
    printf 'Release-candidate harness: FAIL (artifacts preserved at %s)\\n' "$RUN_ROOT" >&2
  fi
}
trap cleanup EXIT

unavailable() { UNAVAILABLE_REASON="$1"; exit 2; }
fail() { FAILURE_REASON="$1"; printf 'Release-candidate harness: FAIL (%s)\\n' "$1" >&2; exit 1; }
for command in awk cat cargo chmod curl grep head jq mktemp python3 sed seq sleep tail tar; do command -v "$command" >/dev/null 2>&1 || unavailable "missing required command: $command"; done

printf 'Release-candidate harness: building packaged artifact\\n'
if ! PLATPULSE_RELEASE_ARCHIVE="$ARCHIVE" "$ROOT/scripts/package-release.sh" "$PACKAGE_DIR" >"$CLI_LOG" 2>&1; then fail 'release artifact build failed; see preserved CLI log'; fi
[[ -f "$ARCHIVE" ]] || fail 'release artifact was not produced'
mkdir -p "$EXTRACTED"
tar -xzf "$ARCHIVE" -C "$EXTRACTED" || fail 'release artifact could not be unpacked'
SERVER="$EXTRACTED/usr/bin/platpulse-server"
WEB_ROOT="$EXTRACTED/usr/share/platpulse/web"
[[ -x "$SERVER" ]] || fail 'packaged Server binary is missing'
[[ -f "$WEB_ROOT/index.html" ]] || fail 'packaged WebUI index is missing'

ORIGIN_URL="http://127.0.0.1:0"
OWNER_PASSWORD="${PLATPULSE_RC_OWNER_PASSWORD:-rc-owner-password-2026}"
VIEWER_PASSWORD="${PLATPULSE_RC_VIEWER_PASSWORD:-rc-viewer-password-2026}"
OWNER_PASSWORD_FILE="$RUN_ROOT/owner-password"
VIEWER_PASSWORD_FILE="$RUN_ROOT/viewer-password"
printf '%s\\n' "$OWNER_PASSWORD" > "$OWNER_PASSWORD_FILE"
printf '%s\\n' "$VIEWER_PASSWORD" > "$VIEWER_PASSWORD_FILE"
chmod 600 "$OWNER_PASSWORD_FILE" "$VIEWER_PASSWORD_FILE"
BASE_URL=""
mkdir -p "$STATE_DIR"
cat > "$CONFIG" <<EOF
state_dir = "$STATE_DIR"
db_path = "$STATE_DIR/platpulse.db"
pepper_file = "$STATE_DIR/server-pepper"
web_root = "$WEB_ROOT"
listen = "127.0.0.1:0"
public_base_url = "$ORIGIN_URL"
development = true
EOF

printf 'Release-candidate harness: provisioning isolated identities and Networks\\n'
"$SERVER" init --config "$CONFIG" >>"$CLI_LOG" 2>&1 || fail 'isolated Server init failed; see preserved CLI log'
"$SERVER" owner create --config "$CONFIG" --username rc-owner <"$OWNER_PASSWORD_FILE" >>"$CLI_LOG" 2>&1 || fail 'Owner provisioning failed; see preserved CLI log'
"$SERVER" viewer create --config "$CONFIG" --username rc-viewer <"$VIEWER_PASSWORD_FILE" >>"$CLI_LOG" 2>&1 || fail 'Viewer provisioning failed; see preserved CLI log'
"$SERVER" network create --config "$CONFIG" --key platon-mainnet --display-name 'PlatON Mainnet' --genesis-hash "0x$(printf 'a%.0s' {1..64})" --chain-id 210425 --p2p-network-id 210425 --address-hrp lat >>"$CLI_LOG" 2>&1 || fail 'Network provisioning failed; see preserved CLI log'
"$SERVER" network create --config "$CONFIG" --key platon-testnet --display-name 'PlatON Testnet' --genesis-hash "0x$(printf 'b%.0s' {1..64})" --chain-id 210426 --p2p-network-id 210426 --address-hrp lat >>"$CLI_LOG" 2>&1 || fail 'Network provisioning failed; see preserved CLI log'

printf 'Release-candidate harness: starting packaged Server\\n'
"$SERVER" serve --config "$CONFIG" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
PORT=""
for _ in $(seq 1 100); do
  PORT="$(sed -nE 's/.*listening on 127\\.0\\.0\\.1:([0-9]+).*/\\1/p' "$SERVER_LOG" | tail -n 1)"
  if [[ -n "$PORT" ]]; then BASE_URL="http://127.0.0.1:$PORT"; break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || fail 'packaged Server exited before binding'
  sleep 0.1
done
[[ -n "$BASE_URL" ]] || fail 'packaged Server did not report a bound port'
for _ in $(seq 1 100); do
  live_status="$(curl -sS --connect-timeout 2 --max-time 5 -o "$RUN_ROOT/live.body" -w '%{http_code}' "$BASE_URL/health/live" 2>/dev/null || true)"
  [[ "$live_status" == 200 ]] && break
  kill -0 "$SERVER_PID" 2>/dev/null || fail 'packaged Server exited before health check'
  sleep 0.1
done
[[ "$live_status" == 200 ]] || fail 'health/live did not become ready'

request_id() {
  local id
  id="$(awk -F': ' 'tolower($1) == "x-request-id" { gsub("\\\\r", "", $2); print $2; exit }' "$1")"
  if [[ -n "$id" ]]; then printf '%s %s\\n' "${1##*/}" "$id" >> "$REQUEST_IDS"; fi
  printf '%s' "$id"
}
request() {
  local name="$1"; shift
  local headers="$RUN_ROOT/$name.headers" body="$RUN_ROOT/$name.body" status="$RUN_ROOT/$name.status"
  curl -sS --connect-timeout 2 --max-time 10 -D "$headers" -o "$body" -w '%{http_code}' "$@" >"$status" || fail "HTTP request failed: $name"
  LAST_REQUEST_ID="$(request_id "$headers")"
  [[ "$(cat "$status")" == 200 ]] || fail "HTTP request $name returned $(cat "$status") (request $LAST_REQUEST_ID)"
}

printf 'Release-candidate harness: checking health, WebUI, and human boundaries\\n'
request ready "$BASE_URL/health/ready"
jq -e '.status == "ready"' "$RUN_ROOT/ready.body" >/dev/null || fail "health/ready was not ready (request $LAST_REQUEST_ID)"
request web-index "$BASE_URL/"
grep -q '<div id="root"' "$RUN_ROOT/web-index.body" || fail "packaged WebUI index was not served (request $LAST_REQUEST_ID)"
WEB_ASSET_PATH="$(grep -oE '/assets/[^" ]+' "$RUN_ROOT/web-index.body" | head -n 1)"
[[ "$WEB_ASSET_PATH" == /assets/* ]] || fail "packaged WebUI asset reference was missing (request $LAST_REQUEST_ID)"
request web-asset "$BASE_URL$WEB_ASSET_PATH"

login() {
  local role="$1" username="$2" password_file="$3" cookie_file="$4"
  local login_json="$RUN_ROOT/$role-login.json" headers="$RUN_ROOT/$role-login.headers" body="$RUN_ROOT/$role-login.body" status="$RUN_ROOT/$role-login.status"
  jq -n --arg username "$username" --rawfile password "$password_file" '{username: $username, password: ($password | rtrimstr("\\n"))}' > "$login_json"
  curl -sS --connect-timeout 2 --max-time 10 -D "$headers" -c "$cookie_file" -o "$body" -H 'Content-Type: application/json' -H "Origin: $ORIGIN_URL" --data-binary "@$login_json" "$BASE_URL/api/public/v1/login" -w '%{http_code}' >"$status" || fail "$role login request failed"
  LAST_REQUEST_ID="$(request_id "$headers")"
  [[ "$(cat "$status")" == 200 ]] || fail "$role login failed (request $LAST_REQUEST_ID)"
  jq -e --arg expected_role "$role" '.session.role == $expected_role and (.csrfToken | length > 0)' "$body" >/dev/null || fail "$role login response was invalid (request $LAST_REQUEST_ID)"
}
login owner rc-owner "$OWNER_PASSWORD_FILE" "$OWNER_COOKIE"
login viewer rc-viewer "$VIEWER_PASSWORD_FILE" "$VIEWER_COOKIE"
request public-networks -b "$VIEWER_COOKIE" "$BASE_URL/api/public/v1/networks"
jq -e 'type == "array"' "$RUN_ROOT/public-networks.body" >/dev/null || fail "Viewer REST projection was invalid (request $LAST_REQUEST_ID)"

printf 'Release-candidate harness: opening authorized Admin SSE\\n'
curl -sS --connect-timeout 2 -N -D "$SSE_HEADERS" -o "$SSE_OUTPUT" -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/events" 2>"$RUN_ROOT/sse.stderr" &
SSE_PID=$!
for _ in $(seq 1 50); do
  grep -q 'HTTP/.* 200' "$SSE_HEADERS" 2>/dev/null && break
  kill -0 "$SSE_PID" 2>/dev/null || fail 'authorized Admin SSE exited before connecting'
  sleep 0.1
done
grep -q 'HTTP/.* 200' "$SSE_HEADERS" || fail 'authorized Admin SSE did not return HTTP 200'
SSE_REQUEST_ID="$(request_id "$SSE_HEADERS")"
[[ -n "$SSE_REQUEST_ID" ]] && LAST_REQUEST_ID="$SSE_REQUEST_ID"

printf 'Release-candidate harness: enrolling Agent and submitting two-Node report\\n'
"$SERVER" agent create-enrollment-token --config "$CONFIG" >"$RUN_ROOT/enrollment-output" 2>>"$CLI_LOG" || fail 'Enrollment token provisioning failed; see preserved CLI log'
ENROLLMENT_TOKEN="$(tail -n 1 "$RUN_ROOT/enrollment-output")"
[[ "$ENROLLMENT_TOKEN" == pp_enroll_* ]] || fail 'Enrollment token output was invalid'
curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/enroll.headers" -o "$RUN_ROOT/enroll.body" -H "Authorization: Bearer $ENROLLMENT_TOKEN" -w '%{http_code}' -X POST "$BASE_URL/api/agent/v1/enroll" >"$RUN_ROOT/enroll.status" || fail 'Agent enrollment request failed'
unset ENROLLMENT_TOKEN
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/enroll.headers")"
[[ "$(cat "$RUN_ROOT/enroll.status")" == 200 ]] || fail "Agent enrollment failed (request $LAST_REQUEST_ID)"
AGENT_ID="$(jq -r '.agent_id' "$RUN_ROOT/enroll.body")"
AGENT_EPOCH="$(jq -r '.agent_epoch' "$RUN_ROOT/enroll.body")"
AGENT_CREDENTIAL="$(jq -r '.credential' "$RUN_ROOT/enroll.body")"
[[ "$AGENT_ID" != null && -n "$AGENT_ID" ]] || fail "Enrollment did not return an Agent identity (request $LAST_REQUEST_ID)"
[[ "$AGENT_CREDENTIAL" == pp_agent_* ]] || fail "Enrollment did not return an Agent Credential (request $LAST_REQUEST_ID)"
REPORT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
jq --arg agent_id "$AGENT_ID" --arg epoch "$AGENT_EPOCH" --arg report_id "$REPORT_ID" --arg boot_id "$BOOT_ID" '.agent_id = $agent_id | .agent_epoch = ($epoch | tonumber) | .report_id = $report_id | .boot_id = $boot_id | .report_sequence = 1' "$ROOT/crates/platpulse-core/tests/fixtures/report_v1_canonical.json" > "$RUN_ROOT/report.json"
NODE_A_ID="$(jq -r '.inventory.nodes[0].node_id' "$RUN_ROOT/report.json")"
NODE_B_ID="$(jq -r '.inventory.nodes[1].node_id' "$RUN_ROOT/report.json")"
jq -e '.inventory.nodes | length >= 2' "$RUN_ROOT/report.json" >/dev/null || fail 'smoke report fixture does not contain two independent Nodes'
curl -sS --connect-timeout 2 --max-time 10 -D "$RUN_ROOT/report.headers" -o "$RUN_ROOT/report.body" -H "Authorization: Bearer $AGENT_CREDENTIAL" -H 'Content-Type: application/json' --data-binary "@$RUN_ROOT/report.json" -w '%{http_code}' "$BASE_URL/api/agent/v1/reports" >"$RUN_ROOT/report.status" || fail 'AgentReport request failed'
unset AGENT_CREDENTIAL
LAST_REQUEST_ID="$(request_id "$RUN_ROOT/report.headers")"
[[ "$(cat "$RUN_ROOT/report.status")" == 200 ]] || fail "AgentReport failed (request $LAST_REQUEST_ID)"
jq -e --arg report_id "$REPORT_ID" --arg node_a "$NODE_A_ID" --arg node_b "$NODE_B_ID" '.receipt.report_id == $report_id and ([.receipt.nodes[] | select(.current != null) | .node_id] | unique | sort) == ([$node_a, $node_b] | sort) and all(.receipt.nodes[] | select(.current != null); .current == "accepted" or .current == "rejected")' "$RUN_ROOT/report.body" >/dev/null || fail "Report Receipt did not commit both Node dispositions (request $LAST_REQUEST_ID)"

printf 'Release-candidate harness: verifying current projections and invalidation\\n'
request admin-agents -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/agents"
jq -e --arg agent_id "$AGENT_ID" --arg node_a "$NODE_A_ID" --arg node_b "$NODE_B_ID" 'any(.[]; .agent_id == $agent_id and .last_report_sequence == 1 and (([.nodes[].node_id] | unique | sort) == ([$node_a, $node_b] | sort)))' "$RUN_ROOT/admin-agents.body" >/dev/null || fail "Admin projection did not contain both committed Nodes (request $LAST_REQUEST_ID)"
request node-a -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/nodes/$NODE_A_ID"
jq -e --arg node_id "$NODE_A_ID" '.node_id == $node_id and .lifecycle == "active" and .inventory_revision == 7' "$RUN_ROOT/node-a.body" >/dev/null || fail "Node A current projection was not committed (request $LAST_REQUEST_ID)"
request node-b -b "$OWNER_COOKIE" "$BASE_URL/api/admin/v1/nodes/$NODE_B_ID"
jq -e --arg node_id "$NODE_B_ID" '.node_id == $node_id and .lifecycle == "active" and .inventory_revision == 7' "$RUN_ROOT/node-b.body" >/dev/null || fail "Node B current projection was not committed (request $LAST_REQUEST_ID)"
for _ in $(seq 1 50); do
  if grep -q 'event: invalidation' "$SSE_OUTPUT" && grep -q '"resource":"node"' "$SSE_OUTPUT" && grep -q '"revision":1' "$SSE_OUTPUT"; then break; fi
  sleep 0.1
done
if ! grep -q 'event: invalidation' "$SSE_OUTPUT" || ! grep -q '"resource":"node"' "$SSE_OUTPUT" || ! grep -q '"revision":1' "$SSE_OUTPUT"; then fail "authorized Admin SSE did not observe the report-1 Node invalidation (request $LAST_REQUEST_ID)"; fi

printf 'Release-candidate harness: PASS (artifact=%s, request_id=%s)\\n' "$ARCHIVE" "$LAST_REQUEST_ID"
exit 0
