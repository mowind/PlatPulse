#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACT_DIR="${1:-$ROOT/target/release-artifacts}"
[[ -d "$ARTIFACT_DIR" ]] || { echo "artifact directory not found: $ARTIFACT_DIR" >&2; exit 2; }
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/platpulse-package-inspection.XXXXXX")"
trap 'rm -rf "$RUN_ROOT"' EXIT
shopt -s nullglob
DEBS=("$ARTIFACT_DIR"/*.deb)
RPMS=("$ARTIFACT_DIR"/*.rpm)
[[ ${#DEBS[@]} -eq 2 ]] || { echo 'expected Server and Agent DEB packages' >&2; exit 1; }
[[ ${#RPMS[@]} -eq 2 ]] || { echo 'expected Server and Agent RPM packages' >&2; exit 1; }
kind_for_package() { case "${1##*/}" in platpulse-server-*) echo server;; platpulse-agent-*) echo agent;; *) exit 1;; esac; }
smoke_payload() {
  local root="$1" kind="$2"
  "$ROOT/scripts/validate-release.sh" --root "$root" --kind "$kind"
  "$root/usr/bin/platpulse-$kind" --help >/dev/null
  [[ "$kind" != server ]] || "$root/usr/bin/platpulse-server" backup --help >/dev/null
}
command -v dpkg-deb >/dev/null || exit 2
for package in "${DEBS[@]}"; do
  kind="$(kind_for_package "$package")"; package_root="$RUN_ROOT/deb-$kind/root"; control_root="$RUN_ROOT/deb-$kind/control"
  mkdir -p "$package_root" "$control_root"; dpkg-deb -x "$package" "$package_root"; dpkg-deb --control "$package" "$control_root"
  smoke_payload "$package_root" "$kind"
  depends="$(dpkg-deb -f "$package" Depends)"
  for dependency in ca-certificates adduser coreutils libc-bin systemd; do grep -Eq "(^|[, |])$dependency([, |]|$)" <<<"$depends"; done
  grep -q '^getent group platpulse-' "$control_root/postinst"; grep -q '^install -d .* -m 0700 ' "$control_root/postinst"; grep -q '^systemctl daemon-reload' "$control_root/postinst"
  if dpkg-deb --fsys-tarfile "$package" | tar --numeric-owner -tvf - | awk '$2 != "0/0" { bad=1 } END { exit !bad }'; then echo "DEB payload contains a non-root owner: $package" >&2; exit 1; fi
done
for command in rpm rpm2cpio cpio; do command -v "$command" >/dev/null || exit 2; done
for package in "${RPMS[@]}"; do
  kind="$(kind_for_package "$package")"; package_root="$RUN_ROOT/rpm-$kind/root"; mkdir -p "$package_root"
  rpm2cpio "$package" | (cd "$package_root" && cpio -idm --quiet); smoke_payload "$package_root" "$kind"
  requires="$(rpm -qp --requires "$package")"
  for dependency in ca-certificates shadow-utils coreutils glibc-common systemd; do grep -qx "$dependency" <<<"$requires"; done
  scripts="$(rpm -qp --scripts "$package")"; grep -q '^getent group platpulse-' <<<"$scripts"; grep -q '^install -d .* -m 0700 ' <<<"$scripts"; grep -q '^systemctl daemon-reload' <<<"$scripts"
  if rpm -qplv "$package" | awk '$3 != "root" || $4 != "root" { bad=1 } END { exit !bad }'; then echo "RPM payload contains a non-root owner: $package" >&2; exit 1; fi
done
printf 'final native package inspection: PASS\n'
