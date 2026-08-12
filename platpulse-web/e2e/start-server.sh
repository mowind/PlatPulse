#!/usr/bin/env bash
# Boot a development-mode platpulse-server with a fresh state directory and
# one Owner plus one Viewer for the Playwright suite. The suite runs against
# the real Server serving the production WebUI build, so login and the
# Home/Admin shells are exercised end to end (design §12.1, §12.2, §14.1).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WEB_DIR="$ROOT/platpulse-web"
PORT="${E2E_PORT:-4173}"
PASSWORD="${PLATPULSE_E2E_PASSWORD:-platpulse-e2e-admin-2026}"
VIEWER_PASSWORD="${PLATPULSE_E2E_VIEWER_PASSWORD:-platpulse-e2e-viewer-2026}"
STATE_DIR="$(mktemp -d /tmp/platpulse-e2e-XXXXXX)"
CONFIG="$STATE_DIR/server.toml"

# Build the production bundle the Server will host.
cd "$WEB_DIR"
npm run build >/dev/null

cat > "$CONFIG" <<EOF
state_dir = "$STATE_DIR"
db_path = "$STATE_DIR/platpulse.db"
pepper_file = "$STATE_DIR/server-pepper"
web_root = "$WEB_DIR/dist"
listen = "127.0.0.1:$PORT"
public_base_url = "http://127.0.0.1:$PORT"
development = true
EOF

cd "$ROOT"
cargo run -q -p platpulse-server -- init --config "$CONFIG" >/dev/null
printf '%s\n' "$PASSWORD" | cargo run -q -p platpulse-server -- owner create --config "$CONFIG" --username admin >/dev/null
printf '%s\n' "$VIEWER_PASSWORD" | cargo run -q -p platpulse-server -- viewer create --config "$CONFIG" --username viewer >/dev/null
exec cargo run -q -p platpulse-server -- serve --config "$CONFIG"
