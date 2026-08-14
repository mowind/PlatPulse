#!/usr/bin/env bash
# Build a relocatable release bundle containing the Server binary and the
# Vite WebUI. The production runtime is only platpulse-server.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_ROOT_INPUT="${1:-$ROOT/target/release-package}"
if [[ "$OUTPUT_ROOT_INPUT" = /* ]]; then
  OUTPUT_ROOT="$OUTPUT_ROOT_INPUT"
else
  OUTPUT_ROOT="$ROOT/$OUTPUT_ROOT_INPUT"
fi
OUTPUT_ROOT="$(realpath -m "$OUTPUT_ROOT")"

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

test -x "$PACKAGE_ROOT/usr/bin/platpulse-server"
test -f "$PACKAGE_ROOT/usr/share/platpulse/web/index.html"
test -d "$PACKAGE_ROOT/usr/share/platpulse/web/assets"
test -n "$(find "$PACKAGE_ROOT/usr/share/platpulse/web/assets" -type f -print -quit)"

rm -f "$ARCHIVE"
tar -C "$PACKAGE_ROOT" -czf "$ARCHIVE" usr
ARCHIVE_LIST="$OUTPUT_ROOT/archive.list"
tar -tzf "$ARCHIVE" > "$ARCHIVE_LIST"
grep -Fxq 'usr/bin/platpulse-server' "$ARCHIVE_LIST"
grep -Fxq 'usr/share/platpulse/web/index.html' "$ARCHIVE_LIST"
grep -Eq '^usr/share/platpulse/web/assets/[^/]+$' "$ARCHIVE_LIST"
rm -f "$ARCHIVE_LIST"

printf 'Release bundle: %s\n' "$ARCHIVE"
printf 'Unpacked tree: %s\n' "$PACKAGE_ROOT"
