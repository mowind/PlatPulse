#!/usr/bin/env bash
# Open or update the ci-red issue for a failed workflow on main or a scheduled
# or manual qualification run (issue #283). One open issue per workflow: a
# repeated failure adds a comment with the new run link instead of a second
# issue, so a red main stays visible until someone closes the issue.
#
# Usage (inside GitHub Actions, with GH_TOKEN and GH_REPO set):
#   scripts/report-ci-red.sh "<workflow display name>"
set -euo pipefail

workflow="${1:?usage: report-ci-red.sh <workflow display name>}"
: "${GH_REPO:?GH_REPO must name the repository}"
run_url="${GITHUB_SERVER_URL:-https://github.com}/${GH_REPO}/actions/runs/${GITHUB_RUN_ID:?GITHUB_RUN_ID must be set}"
title="ci-red: ${workflow} failed"

body="$(cat <<EOF
\`${workflow}\` failed.

- Run: ${run_url}
- Event: ${GITHUB_EVENT_NAME:-unknown}
- Ref: ${GITHUB_REF_NAME:-unknown}
- Commit: ${GITHUB_SHA:-unknown}

Fix the cause in a pull request and close this issue once the workflow is green again.
EOF
)"

existing="$(gh issue list --label ci-red --state open --limit 100 --json number,title \
  --jq ".[] | select(.title == \"${title}\") | .number" | head -n 1)"

if [ -n "$existing" ]; then
  gh issue comment "$existing" --body "$body"
  echo "Commented on existing ci-red issue #${existing}"
else
  gh issue create --title "$title" --label ci-red --body "$body"
fi
