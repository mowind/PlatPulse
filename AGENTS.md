# PlatPulse

## Project

PlatPulse is a Server–Agent–WebUI monitoring suite for PlatON nodes, developed as a **Rust workspace** (Agent/Server) plus a **Node.js/TypeScript frontend** (WebUI).

- Design authority: `docs/design/platpulse.md`
- Domain terminology: root `CONTEXT.md` (use its vocabulary, avoid synonyms it explicitly bans)
- Status: greenfield, pre-implementation. Do not inherit ChainDash architecture, TUI, endpoint failover, or remote-control features.

## Tech stack

- **Rust** (Agent + Server): Tokio, Axum + Tower, Reqwest (Rustls), Serde, SQLx SQLite, Alloy (Agent only), sysinfo, Clap + TOML, tracing, utoipa, Argon2id, time.
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
cargo deny check && cargo audit

# Web (platpulse-web)
npm run lint && npm run typecheck && npm test && npm run build

# OpenAPI: regenerate platpulse-web/src/api/generated/ and verify no diff
```

CI also runs Playwright across fixed projects: `phone-360-touch` (360×800), `phone-390-touch` (390×844), `tablet-768-touch` (768×1024), `desktop-1280` (1280×800).

## Engineering conventions

- **SQLite via SQLx with typed SQL** — no ORM, no per-table repository traits; tests use real temp SQLite. Agent and Server keep independent schemas/migrations.
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
