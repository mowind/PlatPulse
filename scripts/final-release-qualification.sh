#!/usr/bin/env bash
# Final Phase 5 release qualification job.
#
# Runs the complete hardening evidence set against the release candidate and
# assembles one consolidated result that preserves reports, residual risks,
# unavailable checks, and generated-artifact verification. Each check records
# an explicit status; a check that cannot run in this environment is recorded
# as NOT_RUN (or UNAVAILABLE) and is never reported as a pass.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$ROOT/scripts/final-release-qualification.py" "$@"
