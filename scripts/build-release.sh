#!/usr/bin/env bash
# Build the supported Linux release set from source or supplied build outputs.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET=""
OUTPUT="${ROOT}/target/release-artifacts"
VERSION="${PLATPULSE_VERSION:-}"
BINARY_DIR=""
WEB_DIR=""
SKIP_BUILD=0

usage() {
  cat >&2 <<'USAGE'
usage: build-release.sh [options]
  --target <rust-target>     Linux Rust target (default: host target)
  --output <directory>       Artifact directory
  --version <version>        Release version (default: workspace version)
  --binary-dir <directory>   Prebuilt platpulse-agent/server directory
  --web-dir <directory>      Prebuilt WebUI dist directory
  --skip-build               Use --binary-dir and --web-dir without building
USAGE
  exit 2
}
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) TARGET="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    --binary-dir) BINARY_DIR="${2:-}"; shift 2 ;;
    --web-dir) WEB_DIR="${2:-}"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

command -v cargo >/dev/null 2>&1 || { echo 'cargo is required' >&2; exit 2; }
command -v tar >/dev/null 2>&1 || { echo 'tar is required' >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo 'python3 is required' >&2; exit 2; }

if [[ -z "$TARGET" ]]; then
  TARGET="$(rustc -vV | awk '/host:/ {print $2}')"
fi
if [[ -z "$VERSION" ]]; then
  VERSION="$(cargo metadata --manifest-path "$ROOT/Cargo.toml" --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "platpulse-server"))')"
fi
case "$TARGET" in
  x86_64-unknown-linux-gnu) ARCH=x86_64 ;;
  aarch64-unknown-linux-gnu) ARCH=aarch64 ;;
  *) echo "unsupported Linux target: $TARGET (expected x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu)" >&2; exit 2 ;;
esac

if [[ "$OUTPUT" != /* ]]; then OUTPUT="$ROOT/$OUTPUT"; fi
OUTPUT="$(realpath -m "$OUTPUT")"
case "$OUTPUT" in
  "$ROOT/target"/*|/tmp/*) ;;
  *) echo "output directory must be below $ROOT/target or /tmp" >&2; exit 2 ;;
esac
rm -rf "$OUTPUT"
mkdir -p "$OUTPUT/staging/server/root" "$OUTPUT/staging/agent/root"
SERVER_ROOT="$OUTPUT/staging/server/root"
AGENT_ROOT="$OUTPUT/staging/agent/root"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  (cd "$ROOT/platpulse-web" && npm run build)
  (cd "$ROOT" && cargo build --locked --release --target "$TARGET" -p platpulse-agent -p platpulse-server)
fi
if [[ -z "$BINARY_DIR" ]]; then BINARY_DIR="$ROOT/target/$TARGET/release"; fi
if [[ -z "$WEB_DIR" ]]; then WEB_DIR="$ROOT/platpulse-web/dist"; fi
[[ -x "$BINARY_DIR/platpulse-server" ]] || { echo "missing platpulse-server in $BINARY_DIR" >&2; exit 1; }
[[ -x "$BINARY_DIR/platpulse-agent" ]] || { echo "missing platpulse-agent in $BINARY_DIR" >&2; exit 1; }
[[ -f "$WEB_DIR/index.html" ]] || { echo "missing WebUI index in $WEB_DIR" >&2; exit 1; }
[[ -d "$WEB_DIR/assets" ]] || { echo "missing WebUI assets in $WEB_DIR" >&2; exit 1; }

install -Dm755 "$BINARY_DIR/platpulse-server" "$SERVER_ROOT/usr/bin/platpulse-server"
install -Dm755 "$BINARY_DIR/platpulse-agent" "$AGENT_ROOT/usr/bin/platpulse-agent"
mkdir -p "$SERVER_ROOT/usr/share/platpulse/web" "$AGENT_ROOT/usr/share/doc/platpulse-agent"
cp -a "$WEB_DIR/." "$SERVER_ROOT/usr/share/platpulse/web/"
install -Dm644 "$ROOT/crates/platpulse-server/server.example.toml" "$SERVER_ROOT/etc/platpulse/server.example.toml"
install -Dm644 "$ROOT/crates/platpulse-agent/agent.example.toml" "$AGENT_ROOT/etc/platpulse-agent/agent.toml.example"
install -Dm644 "$ROOT/docs/deployment.md" "$SERVER_ROOT/usr/share/doc/platpulse-server/deployment.md"
install -Dm644 "$ROOT/LICENSE" "$SERVER_ROOT/usr/share/doc/platpulse-server/LICENSE"
install -Dm644 "$ROOT/docs/deployment.md" "$AGENT_ROOT/usr/share/doc/platpulse-agent/deployment.md"
install -Dm644 "$ROOT/LICENSE" "$AGENT_ROOT/usr/share/doc/platpulse-agent/LICENSE"
install -Dm644 "$ROOT/release/examples/Caddyfile" "$SERVER_ROOT/usr/share/doc/platpulse-server/examples/Caddyfile"
install -Dm644 "$ROOT/release/compose/server.compose.yml" "$SERVER_ROOT/usr/share/doc/platpulse-server/examples/compose.yml"
install -Dm644 "$ROOT/release/compose/server.toml" "$SERVER_ROOT/usr/share/doc/platpulse-server/examples/compose-server.toml"
install -Dm644 "$ROOT/release/geo/geoipupdate.compose.yml" "$SERVER_ROOT/usr/share/doc/platpulse-server/examples/geoipupdate.compose.yml"
for unit in platpulse-server.service platpulse-backup.service platpulse-backup.timer; do
  install -Dm644 "$ROOT/release/systemd/$unit" "$SERVER_ROOT/usr/lib/systemd/system/$unit"
done
install -Dm644 "$ROOT/release/systemd/platpulse-agent.service" "$AGENT_ROOT/usr/lib/systemd/system/platpulse-agent.service"

"$ROOT/scripts/validate-release.sh" --root "$SERVER_ROOT" --kind server
"$ROOT/scripts/validate-release.sh" --root "$AGENT_ROOT" --kind agent

export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || date +%s)}"
export TZ=UTC
export LC_ALL=C
find "$SERVER_ROOT" "$AGENT_ROOT" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
make_archive() {
  local kind="$1" root="$2"
  local archive="$OUTPUT/platpulse-$kind-$VERSION-linux-$ARCH.tar.gz"
  tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner -C "$root" -czf "$archive" usr etc
}
make_archive server "$SERVER_ROOT"
make_archive agent "$AGENT_ROOT"

build_deb() {
  local kind="$1" root="$2"
  local package_root="$OUTPUT/staging/deb-$kind"
  local package="$OUTPUT/platpulse-$kind-$VERSION-$ARCH.deb"
  command -v dpkg-deb >/dev/null 2>&1 || return 0
  rm -rf "$package_root"
  cp -a "$root" "$package_root"
  mkdir -p "$package_root/DEBIAN"
  local deb_arch=amd64
  [[ "$ARCH" == aarch64 ]] && deb_arch=arm64
  local deb_depends="ca-certificates, adduser, coreutils, libc-bin, systemd | systemd-sysv"
  cat > "$package_root/DEBIAN/control" <<EOF
Package: platpulse-$kind
Version: $VERSION
Section: net
Priority: optional
Architecture: $deb_arch
Depends: $deb_depends
Maintainer: PlatPulse maintainers <mowind@users.noreply.github.com>
Description: PlatPulse $kind
 Server–Agent–WebUI monitoring suite for PlatON Nodes.
EOF
  if [[ "$kind" == server ]]; then
    cat > "$package_root/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
getent group platpulse-server >/dev/null || addgroup --system platpulse-server
id -u platpulse-server >/dev/null 2>&1 || adduser --system --ingroup platpulse-server --no-create-home --home /nonexistent --shell /usr/sbin/nologin platpulse-server
install -d -o platpulse-server -g platpulse-server -m 0700 /var/lib/platpulse /var/backups/platpulse /etc/platpulse/secrets
systemctl daemon-reload >/dev/null 2>&1 || true
EOF
  else
    cat > "$package_root/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
getent group platpulse-agent >/dev/null || addgroup --system platpulse-agent
id -u platpulse-agent >/dev/null 2>&1 || adduser --system --ingroup platpulse-agent --no-create-home --home /nonexistent --shell /usr/sbin/nologin platpulse-agent
install -d -o platpulse-agent -g platpulse-agent -m 0700 /var/lib/platpulse-agent
systemctl daemon-reload >/dev/null 2>&1 || true
EOF
  fi
  chmod 755 "$package_root/DEBIAN/postinst"
  dpkg-deb --build --root-owner-group "$package_root" "$package" >/dev/null
}
DEB_STATUS=unavailable
if command -v dpkg-deb >/dev/null 2>&1; then
  build_deb server "$SERVER_ROOT"
  build_deb agent "$AGENT_ROOT"
  DEB_STATUS=produced
fi

build_rpm() {
  local kind="$1" root="$2"
  local top="$OUTPUT/rpm-$kind"
  local package_root="$OUTPUT/staging/rpm-$kind"
  local spec="$OUTPUT/rpm-$kind.spec"
  rm -rf "$top" "$package_root"
  mkdir -p "$top/BUILD" "$top/RPMS" "$top/SOURCES" "$top/SPECS" "$top/SRPMS"
  cp -a "$root/." "$package_root"
  local rpm_requires="ca-certificates, shadow-utils, coreutils, glibc-common, systemd"
  cat > "$spec" <<EOF
Name: platpulse-$kind
Version: $VERSION
Release: 1
Summary: PlatPulse $kind
License: MIT
Requires: $rpm_requires
BuildArch: $([[ "$ARCH" == aarch64 ]] && echo aarch64 || echo x86_64)
%description
PlatPulse Server–Agent–WebUI monitoring suite for PlatON Nodes.
%install
rm -rf %{buildroot}
cp -a $package_root/. %{buildroot}/
EOF
  if [[ "$kind" == server ]]; then
    cat >> "$spec" <<'EOF'
%post
getent group platpulse-server >/dev/null || groupadd --system platpulse-server
id -u platpulse-server >/dev/null 2>&1 || useradd --system --gid platpulse-server --home-dir /nonexistent --shell /sbin/nologin platpulse-server
install -d -o platpulse-server -g platpulse-server -m 0700 /var/lib/platpulse /var/backups/platpulse /etc/platpulse/secrets
systemctl daemon-reload >/dev/null 2>&1 || true
%files
/usr/bin/platpulse-server
/etc/platpulse
/usr/share/platpulse
/usr/share/doc/platpulse-server
/usr/lib/systemd/system/platpulse-server.service
/usr/lib/systemd/system/platpulse-backup.service
/usr/lib/systemd/system/platpulse-backup.timer
EOF
  else
    cat >> "$spec" <<'EOF'
%post
getent group platpulse-agent >/dev/null || groupadd --system platpulse-agent
id -u platpulse-agent >/dev/null 2>&1 || useradd --system --gid platpulse-agent --home-dir /nonexistent --shell /sbin/nologin platpulse-agent
install -d -o platpulse-agent -g platpulse-agent -m 0700 /var/lib/platpulse-agent
systemctl daemon-reload >/dev/null 2>&1 || true
%files
/usr/bin/platpulse-agent
/etc/platpulse-agent
/usr/share/doc/platpulse-agent
/usr/lib/systemd/system/platpulse-agent.service
EOF
  fi
  rpmbuild --define "_topdir $top" --define "_builddir $top/BUILD" --define "_rpmdir $top/RPMS" --define "_srcrpmdir $top/SRPMS" --define "_sourcedir $top/SOURCES" --define "_specdir $top/SPECS" --define "_buildrootdir $top/BUILDROOT" --define "_source_date_epoch $SOURCE_DATE_EPOCH" --define "clamp_mtime_to_source_date_epoch 1" --define "use_source_date_epoch_as_buildtime 1" -bb "$spec" >/dev/null
  find "$top/RPMS" -type f -name '*.rpm' -exec cp {} "$OUTPUT/" \;
}
RPM_STATUS=unavailable
if command -v rpmbuild >/dev/null 2>&1; then
  build_rpm server "$SERVER_ROOT"
  build_rpm agent "$AGENT_ROOT"
  RPM_STATUS=produced
fi
printf 'deb=%s\nrpm=%s\n' "$DEB_STATUS" "$RPM_STATUS" > "$OUTPUT/package-results.txt"

SBOM="$OUTPUT/platpulse-release-$VERSION-linux-$ARCH.spdx.json"
if command -v syft >/dev/null 2>&1; then
  SBOM_CONTEXT="$OUTPUT/sbom-context"
  SBOM_RAW="$OUTPUT/.sbom.raw.json"
  rm -rf "$SBOM_CONTEXT"
  mkdir -p "$SBOM_CONTEXT/platpulse-web"
  cp -a "$OUTPUT/staging" "$SBOM_CONTEXT/staging"
  cp "$ROOT/Cargo.toml" "$ROOT/Cargo.lock" "$SBOM_CONTEXT/"
  cp "$ROOT/platpulse-web/package.json" "$ROOT/platpulse-web/package-lock.json" "$SBOM_CONTEXT/platpulse-web/"
  syft "dir:$SBOM_CONTEXT" -o spdx-json > "$SBOM_RAW"
  python3 - "$SBOM_RAW" "$SBOM" "$VERSION" "$ARCH" "$SOURCE_DATE_EPOCH" <<'PY'
import datetime, json, pathlib, sys
source, output = map(pathlib.Path, sys.argv[1:3])
version, arch, epoch = sys.argv[3:6]
doc = json.loads(source.read_text())
doc["creationInfo"]["created"] = datetime.datetime.fromtimestamp(int(epoch), datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
doc["documentNamespace"] = f"https://github.com/mowind/PlatPulse/releases/{version}/{arch}/sbom"
output.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")
PY
  rm -rf "$SBOM_CONTEXT" "$SBOM_RAW"
elif [[ "${PLATPULSE_SKIP_SBOM:-0}" -eq 1 ]]; then
  printf 'sbom=skipped-fixture; not releasable\n' > "$OUTPUT/sbom-results.txt"
else
  echo 'syft is required for a dependency-aware release SBOM' >&2
  exit 2
fi

AUDIT_FAILED=0
{
  printf 'PlatPulse release audit evidence\n'
  printf 'version=%s\narch=%s\ntarget=%s\n' "$VERSION" "$ARCH" "$TARGET"
  if [[ "$SKIP_BUILD" -eq 1 || "${PLATPULSE_SKIP_AUDIT:-0}" -eq 1 ]]; then
    printf 'cargo deny: skipped for fixture/harness build\n'
    printf 'cargo audit: skipped for fixture/harness build\n'
    printf 'npm audit: skipped for fixture/harness build\n'
  else
    if command -v cargo-deny >/dev/null 2>&1; then
      if ! (cd "$ROOT" && cargo deny check); then printf 'cargo deny exited non-zero\n'; AUDIT_FAILED=1; fi
    else printf 'cargo deny: unavailable in build environment\n'; AUDIT_FAILED=1; fi
    if command -v cargo-audit >/dev/null 2>&1; then
      if ! (cd "$ROOT" && cargo audit --ignore RUSTSEC-2023-0071); then printf 'cargo audit exited non-zero\n'; AUDIT_FAILED=1; fi
    else printf 'cargo audit: unavailable in build environment\n'; AUDIT_FAILED=1; fi
    if command -v npm >/dev/null 2>&1; then
      if ! (cd "$ROOT/platpulse-web" && npm audit --audit-level=critical); then printf 'npm audit exited non-zero\n'; AUDIT_FAILED=1; fi
    else printf 'npm audit: unavailable in build environment\n'; AUDIT_FAILED=1; fi
  fi
} > "$OUTPUT/audit-results.txt" 2>&1
if [[ "$AUDIT_FAILED" -ne 0 ]]; then
  cat "$OUTPUT/audit-results.txt" >&2
  echo 'release dependency audit gate failed' >&2
  exit 1
fi

( cd "$OUTPUT" && find . -maxdepth 1 -type f \( -name '*.tar.gz' -o -name '*.deb' -o -name '*.rpm' -o -name '*.spdx.json' -o -name 'audit-results.txt' -o -name 'package-results.txt' -o -name 'sbom-results.txt' \) -printf '%P\n' | sort | xargs -r sha256sum ) > "$OUTPUT/SHA256SUMS"

printf 'Native package results: deb=%s rpm=%s\n' "$DEB_STATUS" "$RPM_STATUS"
printf 'Release artifacts: %s\n' "$OUTPUT"
