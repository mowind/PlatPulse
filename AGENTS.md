# PlatPulse

## Project

PlatPulse is a Server–Agent–WebUI monitoring suite for PlatON nodes, developed as a **Rust workspace** (Agent/Server) plus a **Node.js/TypeScript frontend** (WebUI).

- Design authority: `docs/design/platpulse.md` (an index; its sections live in `docs/design/platpulse/`, each keeping its number)
- Domain terminology: root `CONTEXT.md` (use its vocabulary, avoid synonyms it explicitly bans)
- Status: v0.2.0 released; 0.3.0 adds the PostgreSQL Storage Backend (ADR 0013). Keep ChainDash architecture, TUI, endpoint failover, and remote-control features out of PlatPulse.

## Tech stack

- **Rust** (Agent + Server): Tokio, Axum + Tower, Reqwest (Rustls), Serde, SQLx SQLite (Agent and Server) and PostgreSQL (Server only, ADR 0013), Alloy (Agent only), sysinfo, Clap + TOML, tracing, utoipa, Argon2id, time.
- **Node.js** (WebUI): React, TypeScript strict, Vite, React Router, TanStack Query, native EventSource and fetch.
- No ORM, gRPC, Kafka, NATS, Redis, workflow engine, or global DI container. Dependencies are injected through explicit constructors.

## Workspace layout

```text
Cargo.toml
crates/
├── platpulse-core/     # AgentReport v1, wire types, Observation Envelope, Block Summary, History Gap
├── platpulse-agent/    # config/CLI, collectors, Agent Runtime (Node Supervisor role), AgentStore, report sender
└── platpulse-server/   # HTTP/SSE, auth, Report Ingestion, SQLite projections, alerts, web assets
platpulse-web/          # React SPA; generated API client lives in src/api/generated/
```

- Binary crates use a thin `main.rs` + testable `lib.rs`.
- Do not pre-create shallow crates (`platpulse-db`, `platpulse-api`, …) or empty abstractions/schema for future phases.

## Commands

```bash
# Rust (Agent/Server)
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
# clippy::unwrap_used is on: production code handles the None/Err (or uses expect with the invariant);
# #[test] code may unwrap (clippy.toml), and an integration test file whose helpers unwrap allows it at the top.
cargo test --workspace
# Server tests pick their Storage Backend through the shared helper platpulse_server::test_support (ADR 0013).
# SQLite remains the default (PLATPULSE_TEST_STORAGE=sqlite); PostgreSQL clones a fresh database per test
# from a migrated template and never falls back to SQLite, so it needs a reachable server:
# PLATPULSE_TEST_STORAGE=postgres PLATPULSE_TEST_POSTGRES_URL=postgres://user:pass@host:5432/postgres cargo test --workspace
# A SQLite-only test is annotated `#[cfg_attr(storage_postgres, ignore = "<reason>")]`; crates/platpulse-server/build.rs
# turns PLATPULSE_TEST_STORAGE into that cfg so a PostgreSQL run reports the test as ignored with its reason, never passed.
cargo deny check && cargo audit   # ignores: .cargo/audit.toml (reasons in deny.toml)

# Design references: "design §X.Y" = docs/design/platpulse.md, "webui.md §X.Y" = docs/design/webui.md
# (both are indexes; their sections live under docs/design/platpulse/ and docs/design/webui/, each keeping its number)
python3 scripts/check-design-refs.py

# Logging discipline: no bare eprintln!/dbg!, stdout only in main.rs/cli.rs, log fields from the vocabulary (docs/design/logging.md)
python3 scripts/check-logging.py

# Counts that may only go down (scripts/baselines.json): `issue #N` in crates/*/src and platpulse-web/src,
# and `as any`/`as unknown`/`: any` in platpulse-web/src. Comments state the current contract; history goes in the PR.
python3 scripts/check-baselines.py

# Document length (scripts/doc-budgets.json): 8,000 words per Markdown file, CJK characters counted one each.
# Files already over it are frozen and may only shrink; offset added text or move a section into its own file.
python3 scripts/check-doc-budgets.py

# Screenshots under docs/visual-migration/ are frozen: no added or rewritten images
python3 scripts/check-no-new-screenshots.py --base origin/main

# Committed migrations are frozen (sqlx checksums them at startup): add a new version, never edit, delete or rename one
python3 scripts/check-frozen-migrations.py --base origin/main

# ADRs: numbered 0001.. without gaps, Status line first and short (docs/agents/domain.md)
python3 scripts/check-adr-status.py

# CI lanes a change selects (CI runs only those on a pull request)
git diff --name-only origin/main | python3 scripts/ci-lanes.py

# Web (platpulse-web)
npm run lint && npm run typecheck && npm test && npm run build

# OpenAPI: regenerate platpulse-web/src/api/generated/ and verify no diff
```

CI also runs Playwright, one job per project (desktop-1280 split in two), two workers per job, each worker against its own seeded Server (`e2e/fixtures.ts`: specs import `test` from there), across fixed projects: `phone-360-touch` (360×800), `phone-390-touch` (390×844), `tablet-768-touch` (768×1024), `desktop-1280` (1280×800), `desktop-1440` (1440×900). The 1440 project exists for the Emerald visual-migration acceptance matrix (docs/adr/0002).

## Engineering conventions

- **SQLx with typed SQL** — no ORM, no per-table repository traits. The Server uses SQLx typed SQL on SQLite (default) or PostgreSQL ([ADR 0013](docs/adr/0013-postgresql-server-storage-backend.md); PostgreSQL is a 0.3.0 Storage Backend, issue #255); the Agent is SQLite-only. Tests use real temp SQLite and real PostgreSQL, no mocks. Agent and Server keep independent schemas/migrations.
- **Server domain SQL is written in the portable form** (ADR 0013, #252) — `$N` placeholders (a statement composed from optional `?` fragments is numbered once with `database::number_placeholders`), `ON CONFLICT DO NOTHING` instead of `INSERT OR IGNORE`, `RETURNING` instead of `last_insert_rowid()`, primary-key batching instead of `rowid`, `CASE` instead of scalar `MIN`/`MAX`, no `julianday`, and every column a `DO UPDATE SET` expression reads from the existing row qualified with its table name (`SET n = t.n + 1`) because PostgreSQL sees the target row and the proposed row side by side and rejects an unqualified reference as ambiguous. SQLite-only operations paths (backup, restore, checkpoint, PRAGMA/integrity) are the exception.
- **Server domain code runs on the `sqlx::Any` seam** (ADR 0013, #254) — business modules take `AnyPool`/`AnyConnection`/`Transaction<'_, Any>` and never name `sqlx::sqlite` types; backend-specific operations live under `database/` (and the offline backup/restore/checkpoint/migration tools). `Any` is strict: decode 0/1 flags as `i64` (or `database::NullableFlag` for an `Option<bool>` `FromRow` field), decode a nullable column as `Option<_>`, bind a nullable double as `database::NullableReal` (a bare `Option<f64>` declares `float4` for a missing sample and `float8` for a present one, which poisons the cached PostgreSQL statement), and never ignore a failed statement inside a transaction. Open every transaction with `database::begin` (`BEGIN IMMEDIATE` on SQLite, where a deferred `BEGIN` fails at once with `SQLITE_BUSY` when another connection wrote before its first write; #292); `crates/platpulse-server/clippy.toml` disallows `Pool::begin`.
- **Any Server schema change ships migrations for both backends in the same change** — SQLite and PostgreSQL share one logical schema version and must stay logically equivalent (tables, columns, constraints, indexes). The PostgreSQL baseline migration (crates/platpulse-server/migrations-postgres) is the single schema version they share.
- **`platpulse-core` is I/O-free** — no Axum/SQLx/Alloy dependencies; no Server rows or Public/Admin DTOs.
- **Scope observations per Node, not per Agent** — block, transaction, consensus, peer and error state must never merge into an Agent-level chain view. One Agent may monitor multiple Nodes; one Node has exactly one Endpoint (no failover).
- **Host observation is collected once per Agent** and referenced by Node views; do not duplicate resource accounting.
- **Preserve last-good semantics** — a collection failure updates status/error but never overwrites the last successful value; unknown/stale/never-observed is never shown as `0`, `false`, or Healthy.
- **Server is the trust boundary** — revalidate all Agent-reported fields; never treat Agent input as trusted.
- **Immutable AgentReport + transactional receipt** — reports are persisted before sending and deleted only after the receipt is applied in one transaction; retry uses identical bytes/`report_id`.
- **Block history dedup** — a plain resync replay (`height <= historical_high_watermark`, not an open gap) must not rewrite Block Summaries or re-accumulate counts; only explicit `OpenRecoverableGap` permits backfill below the high-water mark.
- **Home and Admin use different DTOs/route groups** — Public Projection is not a runtime-filtered Admin DTO; visibility filtering happens in the Server query layer.
- **WebUI is mobile-first from Phase 1** — Home and Admin must work on desktop, tablet and mobile; responsive behavior is not a later enhancement.
- **No remote control** — the Server never pushes RPC endpoints, commands, or upgrades to Agents.

## Agent skills

### Change workflow

Every change, including docs, lands on `main` through a squash-merged pull request whose required check is `all-checks-passed`; the work is done when the pull request is merged. Follow `docs/agents/change-workflow.md` when branching, opening a pull request, watching CI, or handling a flaky test.

### Review

`CODING_STANDARDS.md` holds the rules a reviewer checks in the diff (blocking items and the defect classes PlatPulse has hit). Check a change against it before opening its pull request and when reviewing one; the `code-review` skill's Standards review reads it.

### Repository skills

`.agents/skills/` holds the skills specific to this repository: `platpulse-pre-push-checks` picks and runs the local checks a change needs before it is pushed, and `platpulse-schema-change` is the checklist for a Server or Agent schema change (paired SQLite and PostgreSQL migrations).

### Issue tracker

Issues and specs live in GitHub Issues; use the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Domain docs

This is a single-context repository using root `CONTEXT.md` and `docs/adr/`. See `docs/agents/domain.md`.
