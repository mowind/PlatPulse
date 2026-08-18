#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
exec python3 "$ROOT/scripts/release-qualification.py" "$@"
