#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
python3 "$ROOT/scripts/release-qualification.py" --self-test
python3 "$ROOT/scripts/release-qualification.py" --profile "$ROOT/release/qualification/ci.toml" --check-profile
invalid="$(mktemp)"
trap 'rm -f "$invalid"' EXIT
printf '[workload]
agents = 0
' > "$invalid"
if python3 "$ROOT/scripts/release-qualification.py" --profile "$invalid" --check-profile >/dev/null 2>&1; then exit 1; fi
printf 'Release qualification seam tests: PASS
'
