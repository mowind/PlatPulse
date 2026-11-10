# Upgrading to PlatPulse v0.3.0

> **Draft.** v0.3.0 is not released. This guide covers the externally visible changes from tag `v0.2.0` to the current `main` and stays a draft until the `v0.3.0` tag.

v0.3.0 adds PostgreSQL as an opt-in Server Storage Backend ([ADR 0013](../../adr/0013-postgresql-server-storage-backend.md)). An existing SQLite deployment keeps working with its current configuration. The Agent report protocol is unchanged.

## Before you upgrade

1. **Back up the Server.** Stop the Server, run `platpulse-server backup --config /etc/platpulse/server.toml`, and copy the backup, the pepper file and the TLS secret files to protected storage. Keep the v0.2.0 binaries. See "Upgrade and rollback" in [deployment/upgrade-and-rollback.md](../../deployment/upgrade-and-rollback.md).
2. **Back up each Agent Store** (optional, only needed for an Agent rollback). Stop the Agent and copy its `state_db` file (by default `/var/lib/platpulse-agent/agent.db`).
3. **Check disk space.** The Server schema migration rebuilds the raw metric tables (see "Schema migrations"). Make sure the Server's state directory has free space at least the size of the current database file. On Agents that rely on the default Durable Spool size, allow 512 MiB for the Agent Store (see "Breaking changes").
4. **Upgrade the Server first, then the Agents.** The AgentReport wire format is unchanged and `SUPPORTED_PROTOCOL_MAJORS` is `[1, 2]` in both releases (`crates/platpulse-core/src/protocol.rs`), so either version of the Agent reports to either version of the Server. The order matters for one feature: a v0.3.0 Agent re-sends Block Summaries for heights it once declared unrecoverable (block history holes, #281). A v0.3.0 Server accepts them into the hole; a v0.2.0 Server rejects each one with `gap_backfill_outside_open_gap`, and those heights stay holes. Upgrading the Server first avoids that.
5. **Plan the downtime.** The Server is offline while it migrates. Agents keep reports in their Durable Spool and replay them oldest-first after the Server returns; check that the expected downtime fits the Durable Spool capacity ("Durable Spool capacity and the downtime it covers" in [deployment/transport-logs-metrics-and-spool.md](../../deployment/transport-logs-metrics-and-spool.md#durable-spool-capacity-and-the-downtime-it-covers)).

## Breaking changes

### Server schema migrations are one-way (Server schema 75 → 80)

- **What changed:** v0.3.0 applies five Server migrations on its first start. A v0.2.0 Server refuses to start on a migrated database, and `platpulse-server restore` in v0.2.0 refuses a backup taken by v0.3.0 ("newer than this Server supports"). A v0.3.0 Server restores a v0.2.0 backup and migrates it forward.
- **Who is affected:** every Server.
- **Action:** take the pre-upgrade backup described above. Rolling back requires that backup (see "Rollback").

### Default Durable Spool size raised from 64 MiB to 512 MiB (5b390e9, #273)

- **What changed:** the Agent `[spool] max_bytes` default is now `536870912` (512 MiB); preflush starts at 3/4 (384 MiB). Agent migration 0018 updates an existing Agent Store to the new capacity only when it still holds the old 64 MiB default. An explicit `max_bytes` is kept. The valid range (512 KiB – 1 GiB) is unchanged.
- **Who is affected:** Agents whose `agent.toml` does not set `[spool] max_bytes`. During a long Server outage their Agent Store can now grow to about 512 MiB.
- **Action:** make sure the Agent's state directory has the space, or pin the old size with `max_bytes = 67108864` under `[spool]` before you start the v0.3.0 Agent.

### Diagnostic output moved from stdout to stderr (1d38537, #250)

- **What changed:** both binaries now write lifecycle and diagnostic messages as `tracing` logs on stderr. The Server no longer prints its version line and `listening on <addr>` to stdout; it logs `listening` with field `listen=<addr>` on stderr instead. The Agent's final "Agent stopped with shutdown state …" line on stdout is now an `Agent stopped` log on stderr. Logs from third-party libraries default to `warn`.
- **Who is affected:** scripts, health probes or log pipelines that parse the stdout of a running Server or Agent. Under systemd the messages still reach the journal.
- **Action:** read these messages from stderr (or the journal), or use `--log-format json` and match the `listening` event. No action otherwise.

### Node `freshness` replaced by Node Currency in the HTTP API (#345)

- **What changed:** the Node-level `freshness` field is removed from `PublicNode` and from the Admin Node DTOs (`AdminNetworkNode`, `AdminNodeListItem`, `AdminNodeDetail`, `NodeDiagnostic` in the Overview and Agent diagnostics). Each now carries `currency` — `current`, `stale` or `never_observed` (`NodeCurrencyState`) — and its instant, `currencyAsOf` (Public) / `currency_as_of` (Admin): the oldest of the RPC, sync and consensus observation instants, or null while any of them has never been observed. Value-level `freshness` fields (Peers, consensus, Validator) are unchanged.
- **Who is affected:** API clients that read a Node's `freshness`, as an instant or as a `current`/`stale`/`unknown` word. The bundled WebUI is updated.
- **Action:** read `currency` for the state (`never_observed` where `freshness` said `unknown`) and the as-of instant for the time.

### Admin storage failures answer 503 (#413)

- **What changed:** an Admin request that fails on a database read or write now answers 503 with one of two codes: `storage_conflict` for a unique or check constraint violation, or `unavailable` for any other storage failure. Both messages are fixed ("Server storage rejected the change" / "Server database is unavailable"), the failure is logged at `error` under the request id, and a failed existence read no longer answers 404 or a validation error. Admin diagnostics routes that answered 500 on a database read now answer 503. A `password`-hashing failure, previously mislabelled as a database outage, now answers 500 `internal_error`.
- **Who is affected:** API clients, scripts or dashboards that branch on the HTTP status or error code of an Admin mutation or diagnostics read.
- **Action:** treat 503 `storage_conflict` and 503 `unavailable` as a server-side storage failure, not as a missing resource. A mutation answered 503 `unavailable` may already have committed when a post-commit read failed, so reload the resource before retrying.

## Schema migrations

| | v0.2.0 | v0.3.0 |
|---|---|---|
| Server (`SERVER_SCHEMA_VERSION`) | 75 | 81 |
| Agent (`AGENT_SCHEMA_VERSION`) | 17 | 20 |

Server (SQLite, `crates/platpulse-server/migrations/`), applied automatically at startup:

- `0076_storage_migration` — bookkeeping table for `storage migrate`.
- `0077_spool_backlog_age` — two columns on the current Host projection.
- `0078_observation_time` — observation-time columns for Current Projection freshness ([ADR 0014](../../adr/0014-catch-up-freshness-by-observation-time.md)).
- `0079_metric_source_observed_at` — **rebuilds `node_metric_samples` and `host_metric_samples`** to add `source_observed_at` to their key (09cbde1). Every raw metric row is copied into a new table, so the run time and the temporary disk use grow with the stored raw metric history. Do not interrupt the first start.
- `0080_bounded_gap_settlement` — three partial indexes for History Gap settlement.
- `0081_backfill_observation_at` — fills the observation instant of component rows written before `0078`, from their receipt instant. No schema change.

Agent (`crates/platpulse-agent/migrations/`), applied automatically at startup:

- `0018_spool_default_capacity` — new default Durable Spool capacity (above).
- `0019_backfill_backlog` — table for the bounded block history hole backfill.
- `0020_drop_dead_boot_schema` — drops the unused `agent_boots` table and two Boot state values no release wrote. No data an Agent uses changes.

A v0.2.0 Agent refuses an Agent Store at schema 20.

The PostgreSQL migrations (`crates/platpulse-server/migrations-postgres/`, baseline 0075 through 0081) are used only by the new PostgreSQL Storage Backend.

## Configuration changes

- **Server, new optional `[storage]` section.** `backend = "sqlite"` (default) or `"postgres"`, plus `[storage.postgres]` for the connection. Omitting the section keeps SQLite with the existing `db_path`. **Action:** none for existing deployments. See "New optional features".
- **Agent, `[spool] max_bytes` default** — see "Breaking changes".
- No configuration key was renamed or removed in either binary.

## CLI changes

All changes are additions; no command or flag was removed or renamed.

- **Both binaries:** global `--log-level <error|warn|info|debug|trace>` (default `info`, overrides `RUST_LOG`) and `--log-format <text|json>` (default `text`) (1d38537). **Action:** none; see the stdout/stderr change above.
- **Server:** `platpulse-server storage migrate --source <sqlite server.toml> --target <postgres server.toml>`, and `… storage migrate --source … --target … abort` (e90d120). Used only to move to PostgreSQL.
- **Server on PostgreSQL only:** `init`, `backup`, `restore`, `verify-integrity`, `checkpoint create` and `cutover status|resume` refuse to run; back up PostgreSQL with `pg_dump`/`pg_basebackup`. These commands are unchanged on SQLite.

## API changes

No path was removed or renamed in `docs/openapi/openapi.json`; the only removed field is the Node-level `freshness` (see "Breaking changes"). Additions:

- `GET /api/admin/v1/storage` (Owner-only, read-only): the Storage Backend in use (`StorageOverview`).
- `AdminNodeDetail`: `agent_spool_backlog_age`, `block_history_holes`. `HostDiagnostic`: `spool_backlog_age`. `CapacityOverview`: `notApplicableReason` (set on PostgreSQL, where SQLite capacity does not apply).
- `/health/ready` has a new component `storage_ownership`; the storage component is named `sqlite` on SQLite (as before) or `postgres` on PostgreSQL. `platpulse_readiness` gains the same label.
- `NodeCurrencyState`, and `currency` with `currencyAsOf` / `currency_as_of` on the Node DTOs that lost `freshness` (see "Breaking changes").
- New metrics: `platpulse_storage_database_bytes` and `platpulse_history_gap_settlement*` counters. No metric was removed.

Behaviour changes visible to API clients (no action unless you depend on the old behaviour):

- **Freshness follows observation time** (fc2d3c7, ADR 0014). A Node's currency is now judged from the skew-corrected time each observation was taken, not the Server receipt time. While an Agent replays a Durable Spool backlog its Nodes show as Stale instead of "N blocks behind", and a Stale Node is not evaluated by head-lag alerts.
- **One Node Health Summary for Home and Admin** (#345). Public and Admin derive Health from the same rules, so the same Node reads the same `health` and `health_reason` on both. Stale evidence now makes a Node Unknown before any failure it carries (`observations are stale; last RPC observation failed`); Admin now counts a failed process observation and a Network identity mismatch, and Home shows a mismatched Node as Unknown. `health_reason` wording changed to name observations (`RPC observation failed`, `Node is awaiting its first accepted observation`). Admin diagnostics routes answer 503 instead of empty, partial or 500 data when a database read fails.
- **No Session idle timeout** (809dec2, #259). A human Session no longer ends after 12 hours without activity; it ends only at its 7-day absolute lifetime (`expires_at`), on sign-out or on revocation.

## New optional features

### PostgreSQL Storage Backend (opt-in)

The Server can store its database in PostgreSQL 15 or later instead of SQLite ([ADR 0013](../../adr/0013-postgresql-server-storage-backend.md)). It is off unless you set `[storage] backend = "postgres"`. Requirements: a database created with `TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE 'C' LC_CTYPE 'C'`, owned by the Server's role; `host`, `user` and an absolute `password_file` (mode 0600, one line) under `[storage.postgres]`; `sslmode` defaults to `verify-full`. With PostgreSQL, `db_path` and `[capacity]` must be absent, and the `PGSSLCERT`, `PGSSLKEY` and `PGOPTIONS` environment variables (and `PGSSLROOTCERT` when no `ssl_root_cert` is configured) stop the Server from starting. Full reference: "Storage Backend" in [deployment/configuration-and-storage-backend.md](../../deployment/configuration-and-storage-backend.md#storage-backend) and "PostgreSQL Storage Backend" in [deployment/postgresql-storage-backend.md](../../deployment/postgresql-storage-backend.md).

Moving an existing SQLite database uses `storage migrate`. It is **offline and one-way**: the Server must be stopped, the target database empty, and the source must pass its integrity check. On success it writes a `<db_path>.migrated` marker next to the SQLite file, after which the SQLite Storage Backend refuses to start on that file. `abort` is allowed only while the PostgreSQL database has accepted no write since the migration. There is no PostgreSQL → SQLite path. Follow "Storage Backend Migration (issue #257)" in [deployment/upgrade-and-rollback.md](../../deployment/upgrade-and-rollback.md#storage-backend-migration-issue-257).

### Notable new features

- Spool Backlog Age per Agent in Admin and Server Doctor (Doctor warns above 5 minutes).
- Block history holes: Admin Node detail lists them and the Agent backfills them in bounded steps.
- Current freshness by observation time, so a backlog replay no longer looks like Node lag.
- Structured logs with `--log-level`, `--log-format json` and a per-request `request_id`.
- Read-only Storage Backend card in Admin Settings; Server Doctor checks for PostgreSQL.
- Geo country lookups are cached for 7 days.

## Rollback

- **Server:** stop v0.3.0. Because the migrations are one-way, do not start v0.2.0 on the migrated database. Restore the pre-upgrade backup with the v0.2.0 `platpulse-server restore` into a fresh state directory, restore the pepper and TLS secret files with their private permissions, and start v0.2.0. Reports accepted by v0.3.0 after the backup are not in it. v0.3.0 Agents keep reporting to a v0.2.0 Server, so Agents can stay on v0.3.0.
- **Server on PostgreSQL:** there is no way back to SQLite from the migrated data. While PostgreSQL has accepted no write, `storage migrate … abort` undoes the migration; after that, the only way back is the pre-upgrade SQLite backup, which loses everything written to PostgreSQL.
- **Agent:** a v0.2.0 Agent refuses a schema-19 Agent Store. Prefer keeping the Agent on v0.3.0. If you must roll back, stop the Agent and put back the Agent Store copy taken before the upgrade.
