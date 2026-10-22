# PlatPulse

## Project

PlatPulse is a Server–Agent–WebUI monitoring suite for PlatON nodes, developed as a **Rust workspace** (Agent/Server) plus a **Node.js/TypeScript frontend** (WebUI).

- Design authority: `docs/design/platpulse.md`
- Domain terminology: root `CONTEXT.md` (use its vocabulary, avoid synonyms it explicitly bans)
- Status: greenfield, pre-implementation. Do not inherit ChainDash architecture, TUI, endpoint failover, or remote-control features.

## Tech stack

- **Rust** (Agent + Server): Tokio, Axum + Tower, Reqwest (Rustls), Serde, SQLx SQLite (Agent and Server) and PostgreSQL (Server only, ADR 0013), Alloy (Agent only), sysinfo, Clap + TOML, tracing, utoipa, Argon2id, time.
- **Node.js** (WebUI): React, TypeScript strict, Vite, React Router, TanStack Query, native EventSource and fetch.
- No ORM, gRPC, Kafka, NATS, Redis, workflow engine, or global DI container. Dependencies are injected through explicit constructors.

## Workspace layout

```text
Cargo.toml
crates/
├── platpulse-core/     # AgentReport v1, wire types, Observation Envelope, Block Summary, History Gap
├── platpulse-agent/    # config/CLI, collectors, Node Supervisor, AgentStore, report sender
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
cargo test --workspace
# Server tests pick their Storage Backend through the shared helper platpulse_server::test_support (ADR 0013).
# SQLite remains the default (PLATPULSE_TEST_STORAGE=sqlite); PostgreSQL clones a fresh database per test
# from a migrated template and never falls back to SQLite, so it needs a reachable server:
# PLATPULSE_TEST_STORAGE=postgres PLATPULSE_TEST_POSTGRES_URL=postgres://user:pass@host:5432/postgres cargo test --workspace
# A SQLite-only test is annotated `#[cfg_attr(storage_postgres, ignore = "<reason>")]`; crates/platpulse-server/build.rs
# turns PLATPULSE_TEST_STORAGE into that cfg so a PostgreSQL run reports the test as ignored with its reason, never passed.
cargo deny check && cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0253 --ignore RUSTSEC-2026-0381

# Design references: "design §X.Y" = docs/design/platpulse.md, "webui.md §X.Y" = docs/design/webui.md
python3 scripts/check-design-refs.py

# Logging discipline: no bare eprintln!/dbg!, stdout only in main.rs/cli.rs, log fields from the vocabulary (docs/design/logging.md)
python3 scripts/check-logging.py

# Web (platpulse-web)
npm run lint && npm run typecheck && npm test && npm run build

# OpenAPI: regenerate platpulse-web/src/api/generated/ and verify no diff
```

CI also runs Playwright across fixed projects: `phone-360-touch` (360×800), `phone-390-touch` (390×844), `tablet-768-touch` (768×1024), `desktop-1280` (1280×800), `desktop-1440` (1440×900). The 1440 project exists for the Emerald visual-migration acceptance matrix (docs/adr/0002).

## Engineering conventions

- **SQLx with typed SQL** — no ORM, no per-table repository traits. The Server uses SQLx typed SQL on SQLite (default) or PostgreSQL ([ADR 0013](docs/adr/0013-postgresql-server-storage-backend.md); PostgreSQL is a 0.3.0 Storage Backend, issue #255); the Agent is SQLite-only. Tests use real temp SQLite and real PostgreSQL, no mocks. Agent and Server keep independent schemas/migrations.
- **Server domain SQL is written in the portable form** (ADR 0013, #252) — `$N` placeholders (a statement composed from optional `?` fragments is numbered once with `database::number_placeholders`), `ON CONFLICT DO NOTHING` instead of `INSERT OR IGNORE`, `RETURNING` instead of `last_insert_rowid()`, primary-key batching instead of `rowid`, `CASE` instead of scalar `MIN`/`MAX`, no `julianday`, and every column a `DO UPDATE SET` expression reads from the existing row qualified with its table name (`SET n = t.n + 1`) because PostgreSQL sees the target row and the proposed row side by side and rejects an unqualified reference as ambiguous. SQLite-only operations paths (backup, restore, checkpoint, PRAGMA/integrity) are the exception.
- **Server domain code runs on the `sqlx::Any` seam** (ADR 0013, #254) — business modules take `AnyPool`/`AnyConnection`/`Transaction<'_, Any>` and never name `sqlx::sqlite` types; backend-specific operations live under `database/` (and the offline backup/restore/checkpoint/migration tools). `Any` is strict: decode 0/1 flags as `i64` (or `database::NullableFlag` for an `Option<bool>` `FromRow` field), decode a nullable column as `Option<_>`, bind a nullable double as `database::NullableReal` (a bare `Option<f64>` declares `float4` for a missing sample and `float8` for a present one, which poisons the cached PostgreSQL statement), and never ignore a failed statement inside a transaction.
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

### Issue tracker

Issues and specs live in GitHub Issues; use the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Domain docs

This is a single-context repository using root `CONTEXT.md` and `docs/adr/`. See `docs/agents/domain.md`.
