---
name: platpulse-pre-push-checks
description: Choose and run the local checks a PlatPulse change needs before it is pushed, from the CI lanes its changed paths select. Use before pushing a branch or opening a pull request, or when asked which checks a change needs.
---

# PlatPulse pre-push checks

CI is the full verification; locally you run the lanes the change selects, so a push is expected to go green on the first run. `scripts/ci-lanes.py` is the single source of truth for which paths select which lanes: read its lanes, never re-derive them from this file.

## 1. Select the lanes

```bash
git fetch origin
git diff --name-only origin/main...HEAD | python3 scripts/ci-lanes.py
```

Include uncommitted work in the list when it will be part of the push (`git diff --name-only origin/main`). Each `<lane>=true` line is a lane to run below. The repository checks always run.

## 2. Run every selected lane

**Repository checks** (always):

```bash
python3 scripts/check-design-refs.py
python3 scripts/check-logging.py
python3 scripts/check-baselines.py
python3 scripts/check-doc-budgets.py
python3 scripts/check-no-new-screenshots.py --base origin/main
python3 scripts/check-frozen-migrations.py --base origin/main
python3 scripts/check-adr-status.py
```

Add `python3 scripts/<name>.py --self-test` for any of these scripts the change edits, and `python3 scripts/ci-lanes.py --self-test` when the lane rules change.

**rust**:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo run -p platpulse-server --quiet -- --print-openapi > docs/openapi/openapi.json && git diff --exit-code docs/openapi/openapi.json
```

**rust_postgres**: the same `cargo test --workspace` against a real PostgreSQL. Run it when the change touches Server SQL, migrations, `database/`, `test_support.rs` or a `#[cfg_attr(storage_postgres, ignore = …)]` annotation; for other selections, state in the pull request that CI covers it.

```bash
docker run -d --rm --name platpulse-pg -e POSTGRES_PASSWORD=platpulse -p 5432:5432 postgres:15
PLATPULSE_TEST_STORAGE=postgres \
PLATPULSE_TEST_POSTGRES_URL=postgres://postgres:platpulse@127.0.0.1:5432/postgres \
  cargo test --workspace
docker stop platpulse-pg
```

CI also runs PostgreSQL 18; a failure there that 15 does not show is usually a type-declaration difference (see the `sqlx::Any` rules in AGENTS.md).

**web** (in `platpulse-web/`):

```bash
npm run lint && npm run typecheck && npm test && npm run build
npm run generate:api && git diff --exit-code src/api/generated
```

**e2e**: the specs the change affects, on the projects they run on. The global setup builds `platpulse-web/dist` and the Server, then each worker boots its own seeded Server; `E2E_PORT=<port>` instead attaches one worker to a Server already running there.

```bash
cd platpulse-web
npx playwright test e2e/<spec>.spec.ts --project=desktop-1280 --project=phone-390-touch
```

Pick the specs that exercise the changed screens, components or API; run the whole suite only when the change is cross-cutting (layout shell, shared helpers, `e2e/helpers.ts`, the start script). Local runs use zero retries, so a pass is a real pass.

**release_package**: `scripts/test-release-packaging.sh`, then `scripts/package-release.sh` when packaging inputs change (`scripts/build-release.sh`, `docs/deployment.md` and `docs/deployment/`, `LICENSE`, packaging metadata).

**rc_harness**: `scripts/release-candidate-harness.sh` when the change touches release qualification, the Agent binary's runtime behaviour, or packaging; otherwise CI covers it.

## 3. Record what ran

The pull request's Testing section lists each command you ran and its result, and names every selected lane you left to CI with the reason. Done when every selected lane either passed locally or is named there as left to CI.
