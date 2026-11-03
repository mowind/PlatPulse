Part of the [PlatPulse Server deployment guide](../deployment.md): PostgreSQL Storage Backend.

## PostgreSQL Storage Backend

PostgreSQL is the second permanent Storage Backend ([ADR 0013](../adr/0013-postgresql-server-storage-backend.md));
SQLite stays the default and the backend is selected only by the
`[storage] backend` key. That key, the `[storage.postgres]` section, and the
connection rules are in "Storage Backend" under "Configuration"; this chapter
covers what an Operator provisions and what changes at runtime.

### Provisioning the role and the database

The Server never creates a role, a database, or a schema for you. Create the
database with the byte-order collation the schema needs — a database's locale
cannot be changed afterwards, so it has to be right at creation:

```bash
sudo -u postgres psql -v ON_ERROR_STOP=1 -c "CREATE ROLE platpulse LOGIN" -c \
  "CREATE DATABASE platpulse OWNER platpulse TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE 'C' LC_CTYPE 'C'"
```

- `TEMPLATE template0` with explicit `ENCODING`, `LC_COLLATE` and `LC_CTYPE` is
  required. A database cloned from `template1` inherits the cluster locale, and
  the Server refuses to start when any text column of its schema is not collated
  `"C"`, because its comparisons, indexes and `ORDER BY` semantics must match
  SQLite's byte order. A database created with the wrong locale is dropped and
  recreated, not repaired in place.
- `OWNER platpulse` also makes the role the schema owner the Server requires: the
  `public` schema belongs to `pg_database_owner`, and the startup check accepts
  ownership through role membership. The Server migrates into the connection's
  `current_schema()`, so a dedicated schema needs an explicit
  `ALTER SCHEMA <schema> OWNER TO platpulse` and a role-level `search_path`.
- The role needs a password for any `pg_hba.conf` rule that asks for one, which
  is every TCP connection to a default installation (`scram-sha-256`, or `md5`
  on an older cluster). Set it inside `psql` so the plaintext never reaches the
  shell history or the server log, and put the same secret in `password_file`:

  ```bash
  sudo -u postgres psql -c "\password platpulse"   # prompts twice, never echoes
  ```

  A socket-only deployment whose `pg_hba.conf` rule is `peer` needs no password
  check, because the service user and the role have the same name (`platpulse`);
  `password_file` is still required and still has to hold one non-empty line, but
  PostgreSQL never asks for it there. `trust` accepts a connection without
  checking any credential and is not a recommended rule. Whatever the rule, the
  password comes from `password_file` alone and is never sent anywhere else.

Confirm the three facts the Server checks before it starts:

```bash
sudo -u postgres psql -d platpulse -c "SELECT version()" \
  -c "SELECT datcollate, datctype FROM pg_database WHERE datname = 'platpulse'" \
  -c "SELECT pg_get_userbyid(nspowner) FROM pg_namespace WHERE nspname = 'public'"
```

`version()` must be 15 or newer; an older server is refused with "PostgreSQL
server version … is older than the supported minimum 15". `datcollate` and
`datctype` must both be `C`. The schema owner must be the Server role, directly
or through membership; otherwise startup names the remedy ("role "…" does not own
the target schema "…" (owner: "…"); make the Server role the schema owner, for
example: ALTER SCHEMA public OWNER TO platpulse"). The Owner-only Doctor
(`POST /api/admin/v1/doctor` queues a run, `GET` returns the latest report)
reports the same four checks —
`postgres_connection`, `postgres_server_version`, `postgres_collation` and
`postgres_ownership` — on every run, so a server upgrade or a changed ownership
stays visible.

The PostgreSQL baseline migration set in
`crates/platpulse-server/migrations-postgres` is applied by the Server itself on
its first start, and by `storage migrate` when an existing SQLite deployment
moves over — there is no `init` on this backend.

### TLS, certificate trust, and the password file

A local database reached through a Unix-domain socket is the only plaintext
case: `sslmode = "disable"` is accepted only with a socket directory in `host`,
and refused over TCP ("`sslmode = "disable"` is refused over TCP: a remote
PostgreSQL connection is always encrypted (ADR 0013)"). A socket deployment has
to write that key out. The mode is applied to the connection whatever the
transport, and PostgreSQL does not speak TLS on a socket, so the default
`verify-full` fails at startup with "error occurred while attempting to
establish a TLS connection: server does not support TLS".

```toml
[storage.postgres]
host = "db.internal.example"
sslmode = "verify-full"
ssl_root_cert_file = "/etc/platpulse/secrets/postgres-ca.pem"
```

`verify-full` verifies the certificate chain and the server name. `verify-ca`
verifies only the chain, so any certificate that CA ever issued is accepted under
any name; use it only when that CA signs for this one database. There is exactly
one configured CA and no client certificate setting. Without
`ssl_root_cert_file` the chain is checked against the public WebPKI roots the
Server is built with, not against the host's `/etc/ssl/certs`, so an internal or
self-signed CA must be named explicitly. The libpq variables that would override those choices make
the Server refuse to start instead of connecting differently from the file —
`PGSSLCERT`, `PGSSLKEY`, `PGOPTIONS`, and `PGSSLROOTCERT` when no
`ssl_root_cert_file` is configured: "the libpq environment variable(s) … would
change how the Server connects to PostgreSQL; unset them (the Server takes every
PostgreSQL setting from `[storage.postgres]` only)". The remaining `PG*`
variables and `.pgpass` are never consulted. [ADR 0013](../adr/0013-postgresql-server-storage-backend.md)
puts that as libpq's environment being ignored; the Server enforces it literally by
building its own connection options (never `PgConnectOptions::new()`) and refusing
the variables it cannot shadow instead of connecting differently from the file.

The password comes only from `password_file`, which requires an absolute path
("`password_file` is required: the password is never read from `server.toml`,
the environment, or the command line") and follows the same sensitive-file policy
as `pepper_file`: a regular file, owned by the service user, no group or world
access (mode `0600`), no symlink in the path, at most 4096 bytes, exactly one
line. Anything else refuses startup, and an empty file is refused too because SQLx
would otherwise fall back to `.pgpass`. The value is read when the connection is
opened, so a rotation takes effect with the next Server start; it never appears in
a log, an error, or the redacted connection `Debug` output.

### One Server per database, and after an ownership loss

An ownership lock, not a configuration convention, keeps two Servers apart: the
running Server holds a session-level advisory lock on a dedicated ownership
connection, and a second Server pointed at the same database refuses to start
("another Server owns this PostgreSQL database: the session advisory lock is
held; only one Server may serve a database at a time"). The work pool keeps a
single connection and never reconnects after ownership is lost; a transaction
interrupted that way rolls back and Ingestion answers with a retryable 503.

While it serves, the Server proves ownership with a heartbeat on that connection
every `ownership_heartbeat_secs`, bounded by `connect_timeout_secs`. A failure —
the connection closed, or a network partition detected by the next heartbeat — is
final for that process: it logs "PostgreSQL ownership connection lost; the Server
stops without reacquiring ownership", stops writing, reports the
`storage_ownership` readiness component as not ready (so `/health/ready` answers
503), drains for up to 10 s, and exits non-zero with "Storage Backend ownership
lost (…, checkpointed=…)". It never takes the lock again in the running process,
so a former owner cannot write beside a new one.

The shipped unit therefore restarts it: `Restart=on-failure` with
`RestartSec=5s`. Each attempt exits immediately while the other session holds the
lock, and the cycle repeats every five seconds. That loop is a recovery retry, not
failover:

- It is not high availability. Never run a second Server, on another host or as a
  standby unit, against the same database: the loser only produces refused work,
  and the loop keeps producing it.
- Treat the loop as an incident, not as a finished recovery. Find the holder
  (`SELECT pid, application_name, state FROM pg_stat_activity;` and
  `SELECT pid, granted FROM pg_locks WHERE locktype = 'advisory';`), stop the
  unit that should not be running, and confirm the remaining Server serves
  `/health/ready`. A former owner whose work was refused lost nothing durable:
  Agents retry from their Durable Spool.
- To keep the unit from retrying indefinitely, bound it with `StartLimitIntervalSec=`
  and `StartLimitBurst=` so systemd parks it in `failed` instead of restarting.
- Do not remove the lock by hand (`pg_terminate_backend`) while a Server is
  writing: terminate the process that lost ownership instead and let its
  connection close.

### Operations that do not apply on PostgreSQL

The SQLite file tooling refuses this backend by design, and the PostgreSQL
tooling that owns the database replaces it:

| Operation | On PostgreSQL |
| --- | --- |
| `init` | Refused: "…operates on the Server SQLite database file and is not available on the PostgreSQL Storage Backend (ADR 0013)…". Nothing is missing — the baseline is applied on first start, or by `storage migrate`. |
| `backup`, `restore`, `verify-integrity`, `checkpoint create` | Refused with the same message. Use `pg_dump`/`pg_restore`, `pg_basebackup` and `pg_verifybackup`, and rehearse the restore yourself: no packaged backup timer or unit exists for this backend. |
| `cutover status`, `cutover resume` | Refused. `storage migrate` refuses a source whose Inventory conversion is still `awaiting_resume`, so the cutover is completed on SQLite before the move. |
| `[capacity]` | Refused when the configuration loads: "the database disk belongs to the database operator, so the Server neither measures nor protects it (ADR 0013)". Delete the section. The filesystem gauges have no filesystem to measure and
publish no sample; `platpulse_capacity_paused` is still published and reads `0`. |
| `doctor` backup checks | Backup age, backup residue and the SQLite integrity check are reported `not_applicable`; the four `postgres_*` checks represent this backend. |
| `/health/ready` and `platpulse_readiness` | Report the backend-named component `postgres`; the storage component this deployment does not use is omitted from both surfaces and never blocks readiness. |
| Operational metrics | `platpulse_storage_database_bytes` comes from `pg_database_size`; `platpulse_sqlite_page_count`, `platpulse_sqlite_freelist_pages` and `platpulse_sqlite_wal_bytes` have no sample (an unknown is never published as zero), and the pool gauges describe the work connection pool (8 connections). |

### Storage Snapshots on PostgreSQL are unsanitized host data

A PostgreSQL dump, a WAL archive or PITR chain, and a managed-instance snapshot
are all Storage Snapshots: complete copies of the Server's state taken outside the
Server process. They are the supported recovery path on this backend, and they are
never sanitized. A Storage Snapshot holds what the database holds — raw Peer
addresses and RPC Endpoints, credential digests, Owner password hashes, Report
Receipt bodies, and Audit before/after JSON — so it needs the same protection as
the database itself:

- encrypt it at rest and restrict it to the operator's backup path and backup
  role; an unencrypted dump on shared storage is a credential disclosure, not
  just a data copy;
- treat every copy of it as in scope for the same handling, including snapshots a
  managed service takes and stores where you cannot delete them;
- the Server cannot see, delete, rotate, or verify these copies, and neither
  `doctor` nor `verify-integrity` says anything about them: retention and
  destruction belong to the database operator's policy;
- restoring one is a disaster-recovery action, never a rollback (see "Upgrade and
  rollback"): a restored dump would also rewind Agent Receipts and Purge
  barriers.

Moving a SQLite deployment onto this backend is one offline command; its
prerequisites, operator sequence, abort rule and downtime budget are in "Storage
Backend Migration (issue #257)" under "Upgrade and rollback", and the measured
runtime is recorded in [release qualification](../release-qualification.md).
