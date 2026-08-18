#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/platpulse-release-test.XXXXXX")"
trap 'rm -rf "$RUN_ROOT"' EXIT

make_fixture() {
  local root="$1" kind="$2"
  mkdir -p "$root/usr/bin" "$root/usr/lib/systemd/system"
  if [[ "$kind" == server ]]; then
    mkdir -p "$root/etc/platpulse"
    mkdir -p "$root/usr/share/platpulse/web/assets" "$root/usr/share/doc/platpulse-server/examples"
    printf '#!/bin/sh\n' > "$root/usr/bin/platpulse-server"
    chmod 755 "$root/usr/bin/platpulse-server"
    printf '<!doctype html>\n' > "$root/usr/share/platpulse/web/index.html"
    printf 'asset\n' > "$root/usr/share/platpulse/web/assets/app.js"
    printf '# config\n' > "$root/etc/platpulse/server.example.toml"
    printf '# deployment\n' > "$root/usr/share/doc/platpulse-server/deployment.md"
    printf 'MIT License\n' > "$root/usr/share/doc/platpulse-server/LICENSE"
    printf '# caddy\n' > "$root/usr/share/doc/platpulse-server/examples/Caddyfile"
    printf '# compose\n' > "$root/usr/share/doc/platpulse-server/examples/compose.yml"
    printf '# compose config\n' > "$root/usr/share/doc/platpulse-server/examples/compose-server.toml"
    printf '# geo\n' > "$root/usr/share/doc/platpulse-server/examples/geoipupdate.compose.yml"
    printf '[Service]\nUser=platpulse-server\nGroup=platpulse-server\nReadWritePaths=/var/lib/platpulse /var/backups/platpulse\n' > "$root/usr/lib/systemd/system/platpulse-server.service"
    printf '[Service]\nUser=platpulse-server\nGroup=platpulse-server\nExecStart=/usr/bin/platpulse-server backup --config /etc/platpulse/server.toml\nReadWritePaths=/var/lib/platpulse /var/backups/platpulse\n' > "$root/usr/lib/systemd/system/platpulse-backup.service"
    printf '[Timer]\n' > "$root/usr/lib/systemd/system/platpulse-backup.timer"
  else
    mkdir -p "$root/etc/platpulse-agent" "$root/usr/share/doc/platpulse-agent"
    printf '#!/bin/sh\n' > "$root/usr/bin/platpulse-agent"
    chmod 755 "$root/usr/bin/platpulse-agent"
    printf '# config\n' > "$root/etc/platpulse-agent/agent.toml.example"
    printf '# deployment\n' > "$root/usr/share/doc/platpulse-agent/deployment.md"
    printf 'MIT License\n' > "$root/usr/share/doc/platpulse-agent/LICENSE"
    printf '[Service]\nUser=platpulse-agent\nGroup=platpulse-agent\n' > "$root/usr/lib/systemd/system/platpulse-agent.service"
  fi
}

VALIDATOR="$ROOT/scripts/validate-release.sh"
PASS_ROOT="$RUN_ROOT/pass-server"
PASS_AGENT_ROOT="$RUN_ROOT/pass-agent"
make_fixture "$PASS_ROOT" server
make_fixture "$PASS_AGENT_ROOT" agent
"$VALIDATOR" --root "$PASS_ROOT" --kind server
"$VALIDATOR" --root "$PASS_AGENT_ROOT" --kind agent

BAD_SERVER_IDENTITY="$RUN_ROOT/bad-server-identity"
cp -a "$PASS_ROOT" "$BAD_SERVER_IDENTITY"
printf 'User=root\n' >> "$BAD_SERVER_IDENTITY/usr/lib/systemd/system/platpulse-server.service"
if "$VALIDATOR" --root "$BAD_SERVER_IDENTITY" --kind server; then
  echo 'validator accepted a later root User override' >&2
  exit 1
fi

BAD_AGENT_IDENTITY="$RUN_ROOT/bad-agent-identity"
cp -a "$PASS_AGENT_ROOT" "$BAD_AGENT_IDENTITY"
printf 'Group=root\n' >> "$BAD_AGENT_IDENTITY/usr/lib/systemd/system/platpulse-agent.service"
if "$VALIDATOR" --root "$BAD_AGENT_IDENTITY" --kind agent; then
  echo 'validator accepted a later root Group override' >&2
  exit 1
fi

BAD_ROOT="$RUN_ROOT/bad"
cp -a "$PASS_ROOT" "$BAD_ROOT"
printf 'private key\n' > "$BAD_ROOT/etc/platpulse/server-pepper"
if "$VALIDATOR" --root "$BAD_ROOT" --kind server; then
  echo 'validator accepted an unexpected secret file' >&2
  exit 1
fi

BAD_WEB="$RUN_ROOT/bad-web"
cp -a "$PASS_ROOT" "$BAD_WEB"
rm "$BAD_WEB/usr/share/platpulse/web/index.html"
if "$VALIDATOR" --root "$BAD_WEB" --kind server; then
  echo 'validator accepted a missing WebUI index' >&2
  exit 1
fi


BAD_STATE="$RUN_ROOT/bad-state"
cp -a "$PASS_ROOT" "$BAD_STATE"
printf 'sqlite\n' > "$BAD_STATE/var.db"
if "$VALIDATOR" --root "$BAD_STATE" --kind server; then
  echo 'validator accepted live SQLite state' >&2
  exit 1
fi

BAD_SYMLINK="$RUN_ROOT/bad-symlink"
cp -a "$PASS_ROOT" "$BAD_SYMLINK"
ln -s server.example.toml "$BAD_SYMLINK/etc/platpulse/server-link"
if "$VALIDATOR" --root "$BAD_SYMLINK" --kind server; then
  echo 'validator accepted a symlink' >&2
  exit 1
fi

BAD_MODE="$RUN_ROOT/bad-mode"
cp -a "$PASS_ROOT" "$BAD_MODE"
chmod 775 "$BAD_MODE/usr/bin/platpulse-server"
if "$VALIDATOR" --root "$BAD_MODE" --kind server; then
  echo 'validator accepted an unsafe executable mode' >&2
  exit 1
fi

BAD_EMPTY="$RUN_ROOT/bad-empty"
cp -a "$PASS_ROOT" "$BAD_EMPTY"
mkdir -p "$BAD_EMPTY/usr/share/platpulse/unexpected-empty"
if "$VALIDATOR" --root "$BAD_EMPTY" --kind server; then
  echo 'validator accepted an unexpected empty directory' >&2
  exit 1
fi

BIN_DIR="$RUN_ROOT/bin"
WEB_DIR="$RUN_ROOT/web"
OUTPUT_DIR="$RUN_ROOT/output"
mkdir -p "$BIN_DIR" "$WEB_DIR/assets"
printf '#!/bin/sh\nexit 0\n' > "$BIN_DIR/platpulse-server"
printf '#!/bin/sh\nexit 0\n' > "$BIN_DIR/platpulse-agent"
chmod 755 "$BIN_DIR/platpulse-server" "$BIN_DIR/platpulse-agent"
printf '<!doctype html><script src="/assets/app.js"></script>\n' > "$WEB_DIR/index.html"
printf 'asset\n' > "$WEB_DIR/assets/app.js"

"$ROOT/scripts/build-release.sh" \
  --skip-build \
  --target x86_64-unknown-linux-gnu \
  --version 9.8.7 \
  --binary-dir "$BIN_DIR" \
  --web-dir "$WEB_DIR" \
  --output "$OUTPUT_DIR"

test -f "$OUTPUT_DIR/platpulse-server-9.8.7-linux-x86_64.tar.gz"
test -f "$OUTPUT_DIR/platpulse-agent-9.8.7-linux-x86_64.tar.gz"
test -f "$OUTPUT_DIR/SHA256SUMS"
test -f "$OUTPUT_DIR/platpulse-release-9.8.7-linux-x86_64.spdx.json"
test -f "$OUTPUT_DIR/audit-results.txt"
test -f "$OUTPUT_DIR/package-results.txt"
test -f "$OUTPUT_DIR/staging/server/root/usr/share/doc/platpulse-server/deployment.md"
test -f "$OUTPUT_DIR/staging/server/root/usr/share/doc/platpulse-server/LICENSE"
test -f "$OUTPUT_DIR/staging/server/root/usr/share/doc/platpulse-server/examples/Caddyfile"
test -f "$OUTPUT_DIR/staging/server/root/usr/share/doc/platpulse-server/examples/compose.yml"
test -f "$OUTPUT_DIR/staging/server/root/usr/share/doc/platpulse-server/examples/compose-server.toml"
test -f "$OUTPUT_DIR/staging/server/root/usr/share/doc/platpulse-server/examples/geoipupdate.compose.yml"
test -f "$OUTPUT_DIR/staging/server/root/usr/lib/systemd/system/platpulse-backup.timer"
test -f "$OUTPUT_DIR/staging/agent/root/usr/share/doc/platpulse-agent/deployment.md"
test -f "$OUTPUT_DIR/staging/agent/root/usr/share/doc/platpulse-agent/LICENSE"
OVERLAP="$(comm -12 <(cd "$OUTPUT_DIR/staging/server/root" && find . -type f -printf '%P\n' | sort) <(cd "$OUTPUT_DIR/staging/agent/root" && find . -type f -printf '%P\n' | sort))"
[[ -z "$OVERLAP" ]] || { printf 'Server/Agent packages overlap regular files:\n%s\n' "$OVERLAP" >&2; exit 1; }
"$VALIDATOR" --root "$OUTPUT_DIR/staging/server/root" --kind server
"$VALIDATOR" --root "$OUTPUT_DIR/staging/agent/root" --kind agent
"$VALIDATOR" --archive "$OUTPUT_DIR/platpulse-server-9.8.7-linux-x86_64.tar.gz" --kind server

UNSAFE_ROOT="$RUN_ROOT/unsafe"
mkdir -p "$UNSAFE_ROOT"
printf 'unexpected\n' > "$UNSAFE_ROOT/unexpected"
tar -C "$OUTPUT_DIR/staging/server/root" -cf "$UNSAFE_ROOT/forbidden.tar" usr etc
tar -C "$UNSAFE_ROOT" -rf "$UNSAFE_ROOT/forbidden.tar" unexpected
gzip "$UNSAFE_ROOT/forbidden.tar"
if "$VALIDATOR" --archive "$UNSAFE_ROOT/forbidden.tar.gz" --kind server; then
  echo 'validator accepted an unexpected archive member' >&2
  exit 1
fi

BAD_ARCHIVE_ROOT="$RUN_ROOT/bad-archive-mode"
cp -a "$PASS_ROOT" "$BAD_ARCHIVE_ROOT"
chmod 775 "$BAD_ARCHIVE_ROOT/usr/bin/platpulse-server"
tar -C "$BAD_ARCHIVE_ROOT" -czf "$RUN_ROOT/bad-mode.tar.gz" usr etc
if "$VALIDATOR" --archive "$RUN_ROOT/bad-mode.tar.gz" --kind server; then
  echo 'validator accepted an unsafe mode stored in an archive' >&2
  exit 1
fi

printf 'release packaging seam tests: PASS\n'
