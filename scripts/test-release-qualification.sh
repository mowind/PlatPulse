#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
python3 "$ROOT/scripts/release-qualification.py" --self-test
python3 "$ROOT/scripts/release-recovery-rehearsal.py" --self-test
python3 "$ROOT/scripts/final-release-qualification.py" --self-test
python3 "$ROOT/scripts/release-qualification.py" --profile "$ROOT/release/qualification/ci.toml" --check-profile
python3 - "$ROOT/release/qualification/security.toml" <<'PY'
import sys
import tomllib
from pathlib import Path

data = tomllib.loads(Path(sys.argv[1]).read_text())
criteria = data["criterion"]
ids = [item["id"] for item in criteria]
assert len(ids) == len(set(ids)) and len(ids) >= 8
for item in criteria:
    assert item["owner"] and isinstance(item["blocking"], bool)
    assert item["disposition"] in {"pass", "partial", "not_run"}
    if item["blocking"]:
        assert item["disposition"] == "pass"
    assert item.get("evidence") or item.get("risk")
print("Security matrix seam tests: PASS")
PY
invalid="$(mktemp)"
trap 'rm -f "$invalid"' EXIT
printf '[workload]
agents = 0
' > "$invalid"
if python3 "$ROOT/scripts/release-qualification.py" --profile "$invalid" --check-profile >/dev/null 2>&1; then exit 1; fi
printf 'Release qualification seam tests: PASS
'
