#!/usr/bin/env bash
# Compatibility wrapper for the release-candidate harness. It uses the supported
# release builder, then exposes the historical Server root/archive locations.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_ROOT_INPUT="${1:-$ROOT/target/release-package}"
if [[ "$OUTPUT_ROOT_INPUT" = /* ]]; then OUTPUT_ROOT="$OUTPUT_ROOT_INPUT"; else OUTPUT_ROOT="$ROOT/$OUTPUT_ROOT_INPUT"; fi
OUTPUT_ROOT="$(realpath -m "$OUTPUT_ROOT")"
case "$OUTPUT_ROOT" in
  "$ROOT/target"/*) ;;
  *) printf 'output directory must be below %s/target\n' "$ROOT" >&2; exit 2 ;;
esac

VERSION="${PLATPULSE_VERSION:-$(cargo metadata --manifest-path "$ROOT/Cargo.toml" --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "platpulse-server"))')}"
TARGET="$(rustc -vV | awk '/host:/ {print $2}')"
case "$TARGET" in x86_64-unknown-linux-gnu) ARCH=x86_64 ;; aarch64-unknown-linux-gnu) ARCH=aarch64 ;; *) echo "unsupported host target: $TARGET" >&2; exit 2 ;; esac
SET_DIR="$OUTPUT_ROOT/release-set"
PLATPULSE_SKIP_AUDIT=1 PLATPULSE_SKIP_SBOM=1 "$ROOT/scripts/build-release.sh" --target "$TARGET" --version "$VERSION" --output "$SET_DIR"
rm -rf "$OUTPUT_ROOT/root"
cp -a "$SET_DIR/staging/server/root" "$OUTPUT_ROOT/root"
SOURCE_ARCHIVE="$SET_DIR/platpulse-server-${VERSION}-linux-$ARCH.tar.gz"
ARCHIVE="${PLATPULSE_RELEASE_ARCHIVE:-$ROOT/target/platpulse-server-${VERSION}.tar.gz}"
mkdir -p "$(dirname "$ARCHIVE")"
cp "$SOURCE_ARCHIVE" "$ARCHIVE"
printf 'Release bundle: %s\n' "$ARCHIVE"
printf 'Unpacked tree: %s\n' "$OUTPUT_ROOT/root"
