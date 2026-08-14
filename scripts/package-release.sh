#!/usr/bin/env bash
# Build a relocatable release bundle containing the Server binary and the
# Vite WebUI. The production runtime is only platpulse-server.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_ROOT="${1:-$ROOT/target/release-package}"

case "$OUTPUT_ROOT" in
  "$ROOT/target"/*) ;;
  *)
    printf 'output directory must be below %s/target\n' "$ROOT" >&2
    exit 2
    ;;
esac

VERSION="${PLATPULSE_VERSION:-$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; packages = json.load(sys.stdin)["packages"]; print(next(p["version"] for p in packages if p["name"] == "platpulse-server"))')}"
ARCHIVE="$ROOT/target/platpulse-server-${VERSION}.tar.gz"
PACKAGE_ROOT="$OUTPUT_ROOT/root"

rm -rf "$OUTPUT_ROOT"
mkdir -p "$PACKAGE_ROOT/usr/bin" "$PACKAGE_ROOT/usr/share/platpulse"

printf '%s\n' 'Building WebUI…'
(cd "$ROOT/platpulse-web" && npm run build)
printf '%s\n' 'Building Server…'
(cd "$ROOT" && cargo build --release -p platpulse-server)

install -Dm755 "$ROOT/target/release/platpulse-server" \
  "$PACKAGE_ROOT/usr/bin/platpulse-server"
cp -a "$ROOT/platpulse-web/dist" "$PACKAGE_ROOT/usr/share/platpulse/web"
printf '%s\n' "$VERSION" > "$PACKAGE_ROOT/usr/share/platpulse/VERSION"

test -x "$PACKAGE_ROOT/usr/bin/platpulse-server"
test -f "$PACKAGE_ROOT/usr/share/platpulse/web/index.html"
test -d "$PACKAGE_ROOT/usr/share/platpulse/web/assets"

rm -f "$ARCHIVE"
tar -C "$PACKAGE_ROOT" -czf "$ARCHIVE" usr

printf 'Release bundle: %s\n' "$ARCHIVE"
printf 'Unpacked tree: %s\n' "$PACKAGE_ROOT"
