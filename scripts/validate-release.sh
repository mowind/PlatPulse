#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 (--root <directory> | --archive <tar.gz>) --kind <server|agent>" >&2
  exit 2
}
ROOT=""
ARCHIVE=""
KIND=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --root) ROOT="${2:-}"; shift 2 ;;
    --archive) ARCHIVE="${2:-}"; shift 2 ;;
    --kind) KIND="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ "$KIND" == server || "$KIND" == agent ]] || usage
[[ -n "$ROOT" || -n "$ARCHIVE" ]] || usage
[[ -z "$ROOT" || -z "$ARCHIVE" ]] || usage

fail() {
  printf 'release validation failed: %s\n' "$1" >&2
  exit 1
}

expected_mode() {
  local type="$1" path="$2"
  if [[ "$type" == d ]]; then
    printf 'drwxr-xr-x'
  else
    case "$path" in
      usr/bin/platpulse-server|usr/bin/platpulse-agent) printf '%s' '-rwxr-xr-x' ;;
      *) printf '%s' '-rw-r--r--' ;;
    esac
  fi
}

TEMP_ROOT=""
# shellcheck disable=SC2329
cleanup() { [[ -z "$TEMP_ROOT" ]] || rm -rf "$TEMP_ROOT"; }
trap cleanup EXIT

if [[ -n "$ARCHIVE" ]]; then
  [[ -f "$ARCHIVE" ]] || fail "archive does not exist: $ARCHIVE"
  command -v tar >/dev/null 2>&1 || fail 'tar is required for archive validation'
  while IFS= read -r member; do
    member="${member#./}"
    member="${member%/}"
    [[ -n "$member" ]] || continue
    if [[ "$member" = /* || "$member" == *..* || "$member" == *\\* ]]; then
      fail "unsafe archive member: $member"
    fi
  done < <(tar -tzf "$ARCHIVE")
  while IFS=$'\t' read -r mode member; do
    member="${member#./}"
    member="${member%/}"
    [[ -n "$member" ]] || continue
    case "$mode" in
      d*) type=d ;;
      -*) type=f ;;
      *) fail "non-regular archive member: $member" ;;
    esac
    expected="$(expected_mode "$type" "$member")"
    [[ "$mode" == "$expected" ]] || fail "archive member has mode $mode, expected $expected: $member"
  done < <(tar -tvzf "$ARCHIVE" | awk '{print $1 "\t" $NF}')
  TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/platpulse-release-validate.XXXXXX")"
  tar --no-same-owner --no-same-permissions -xzf "$ARCHIVE" -C "$TEMP_ROOT"
  ROOT="$TEMP_ROOT"
else
  [[ -d "$ROOT" ]] || usage
fi
ROOT="$(realpath "$ROOT")"

while IFS= read -r path; do
  relative="${path#"$ROOT"/}"
  case "$relative" in
    usr|etc) ;;
    *) fail "unexpected archive member: $relative" ;;
  esac
done < <(find "$ROOT" -mindepth 1 -maxdepth 1 -print)

non_regular="$(find "$ROOT" -mindepth 1 ! -type f ! -type d -print -quit)"
[[ -z "$non_regular" ]] || fail "non-regular release member: ${non_regular#"$ROOT"/}"
empty_directory="$(find "$ROOT" -mindepth 1 -type d -empty -print -quit)"
[[ -z "$empty_directory" ]] || fail "unexpected empty directory: ${empty_directory#"$ROOT"/}"
forbidden="$(find "$ROOT" -type f \( \
  -iname '*pepper*' -o -iname '*credential*' -o -iname '*token*' -o \
  -iname '*.db' -o -iname '*.db-wal' -o -iname '*.db-shm' -o -iname '*.db-journal' -o \
  -iname '*.mmdb' -o -iname '*.key' -o -iname '*.pem' -o -iname '*.env' -o \
  -iname '*private-key*' -o -iname '*privkey*' \
\) -print -quit)"
[[ -z "$forbidden" ]] || fail "forbidden secret or live-state member: ${forbidden#"$ROOT"/}"

allowed_file() {
  if [[ "$KIND" == server ]]; then
    case "$1" in
      usr/bin/platpulse-server|usr/share/platpulse/web/*|etc/platpulse/server.example.toml|usr/lib/systemd/system/platpulse-server.service|usr/lib/systemd/system/platpulse-backup.service|usr/lib/systemd/system/platpulse-backup.timer|usr/share/doc/platpulse-server/deployment.md|usr/share/doc/platpulse-server/examples/Caddyfile|usr/share/doc/platpulse-server/examples/compose.yml|usr/share/doc/platpulse-server/examples/geoipupdate.compose.yml) return 0 ;;
    esac
  else
    case "$1" in
      usr/bin/platpulse-agent|etc/platpulse-agent/agent.toml.example|usr/lib/systemd/system/platpulse-agent.service|usr/share/doc/platpulse-agent/deployment.md) return 0 ;;
    esac
  fi
  return 1
}
while IFS= read -r path; do
  relative="${path#"$ROOT"/}"
  allowed_file "$relative" || fail "unexpected file: $relative"
  expected=644
  case "$relative" in
    usr/bin/platpulse-server|usr/bin/platpulse-agent) expected=755 ;;
  esac
  actual="$(stat -c '%a' "$path")"
  [[ "$actual" == "$expected" ]] || fail "file must have mode 0$expected, found 0$actual: $relative"
done < <(find "$ROOT" -type f -print)
while IFS= read -r path; do
  relative="${path#"$ROOT"/}"
  actual="$(stat -c '%a' "$path")"
  [[ "$actual" == 755 ]] || fail "directory must have mode 0755, found 0$actual: $relative"
done < <(find "$ROOT" -mindepth 1 -type d -print)

require_file() {
  [[ -f "$ROOT/$1" ]] || fail "missing required file: $1"
}
require_executable() {
  local path="$ROOT/$1"
  [[ -f "$path" && -x "$path" ]] || fail "missing executable: $1"
  [[ "$(stat -c '%a' "$path")" == 755 ]] || fail "executable must have mode 0755: $1"
}

case "$KIND" in
  server)
    require_executable usr/bin/platpulse-server
    require_file usr/share/platpulse/web/index.html
    [[ -d "$ROOT/usr/share/platpulse/web/assets" ]] || fail 'missing WebUI assets directory'
    find "$ROOT/usr/share/platpulse/web/assets" -type f -print -quit | grep -q . || fail 'WebUI assets directory is empty'
    require_file etc/platpulse/server.example.toml
    require_file usr/share/doc/platpulse-server/deployment.md
    require_file usr/lib/systemd/system/platpulse-server.service
    require_file usr/lib/systemd/system/platpulse-backup.service
    require_file usr/lib/systemd/system/platpulse-backup.timer
    grep -q '^User=platpulse-server$' "$ROOT/usr/lib/systemd/system/platpulse-server.service" || fail 'Server unit is not explicitly non-root'
    grep -q '^ReadWritePaths=/var/lib/platpulse /var/backups/platpulse$' "$ROOT/usr/lib/systemd/system/platpulse-server.service" || fail 'Server unit cannot write state and backup artifacts'
    grep -q '^User=platpulse-server$' "$ROOT/usr/lib/systemd/system/platpulse-backup.service" || fail 'backup unit is not explicitly non-root'
    grep -q '^ExecStart=/usr/bin/platpulse-server backup --config /etc/platpulse/server.toml$' "$ROOT/usr/lib/systemd/system/platpulse-backup.service" || fail 'backup unit does not use the sanitized Server backup command'
    grep -q '^ReadWritePaths=/var/lib/platpulse /var/backups/platpulse$' "$ROOT/usr/lib/systemd/system/platpulse-backup.service" || fail 'backup unit cannot update metadata and write artifacts'
    ;;
  agent)
    require_executable usr/bin/platpulse-agent
    require_file etc/platpulse-agent/agent.toml.example
    require_file usr/share/doc/platpulse-agent/deployment.md
    require_file usr/lib/systemd/system/platpulse-agent.service
    grep -q '^User=platpulse-agent$' "$ROOT/usr/lib/systemd/system/platpulse-agent.service" || fail 'Agent unit is not explicitly non-root'
    ;;
esac
printf 'release validation: PASS (%s)\n' "$KIND"
