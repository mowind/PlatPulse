# Change workflow

Every change reaches `main` through a pull request. Branch protection applies to admins too, so `main` accepts no direct pushes; its only required check is `all-checks-passed` (issue #283). A change is **done when its pull request is merged**, not when it is pushed.

## Steps

1. **Branch** from the current `origin/main`: `git fetch origin && git switch -c <type>/<issue>-<slug> origin/main`. When the work needs an unmerged pull request, branch from that pull request's branch instead; once it merges (as one squashed commit), move your commits onto `main` with `git rebase --onto origin/main <old-dependency-tip>`.
2. **Check locally** what the change touches, using the commands in `AGENTS.md`. CI runs the full matrix; `scripts/ci-lanes.py` shows which lanes a set of paths selects (`git diff --name-only origin/main | python3 scripts/ci-lanes.py`).
3. **Open the pull request** with the template in `.github/pull_request_template.md`: title `type(scope,#N): summary`, body `Closes #N`, the Testing section filled with the commands and results you actually ran, and Breaking change answered. A breaking change (configuration, CLI, data format or schema, Agent–Server version compatibility, HTTP API) also adds its operator action to the next release's guide under `docs/upgrade-guide/` in the same pull request.
4. **Enable auto-merge**: `gh pr merge --auto --squash`. Squash is the only merge method; the pull request title becomes the commit on `main`.
5. **Watch CI to the end**: `gh pr checks --watch`. On red, read the failure (`gh run view <run-id> --log-failed`, Playwright traces in the `playwright-test-results-<project>-<n>` artifact), fix it on the same branch, push, and watch again. Repeat until the pull request is merged: `gh pr view --json state` reports `MERGED`.

For parallel work, use a separate worktree and session per issue; each one follows these steps on its own branch.

## Flaky tests

CI retries a Playwright test once to record a second trace, and `failOnFlakyTests` still fails the run, so a test that passes only on retry is red. Each flaky test gets its own issue naming its category:

- **timeout budget**: a heavy flow exceeds the 30 s budget; mark that test `test.slow()` with a comment saying why it is slow. The global timeout stays as it is.
- **race**: the test waits on time instead of state; wait for the explicit state.
- **shared data**: the test depends on state another test changes; give it its own data.

When the flaky test is unrelated to your change, open its issue first; then re-run the failed jobs (`gh run rerun <run-id> --failed`) and link the issue in a pull request comment.

## Screenshots

Acceptance screenshots go into the pull request description or a GitHub Actions artifact. `docs/visual-migration/` keeps its existing images frozen: `scripts/check-no-new-screenshots.py` fails CI on any added or rewritten image there. The opt-in evidence specs and scripts (`*_EVIDENCE=…`, `scripts/*-evidence.mjs`) write to the ignored `platpulse-web/evidence/` directory; attach what a reviewer needs from there.
