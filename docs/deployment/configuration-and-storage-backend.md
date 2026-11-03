Part of the [PlatPulse Server deployment guide](../deployment.md): Configuration.

## Configuration

Copy `crates/platpulse-server/server.example.toml` and set an explicit
`state_dir`, secret paths, and public URL. If `web_root` is omitted, the
Server resolves the installed default `/usr/share/platpulse/web`. A configured
`web_root`, or `serve --web-assets`, takes precedence over that default.

The Server may start when the default or configured WebUI directory is
missing/incomplete, but `/health/ready` reports the `web_assets` component as
not ready with reason `web_assets_missing`. Readiness evaluates eight components:
`storage_ownership`, the storage component named for the backend in use
(`sqlite`, or `postgres` with the PostgreSQL Storage Backend), `owner`,
`web_assets`, `shutdown`, `critical_workers`, `corruption`, and
`inventory_cutover`; any `not_ready` component makes the endpoint return HTTP
503. `/health/live`
only proves that the event loop responds. `init` emits a warning instead of
creating or modifying WebUI files.

```bash
platpulse-server init --config /etc/platpulse/server.toml
platpulse-server owner create \
  --config /etc/platpulse/server.toml \
  --username admin
platpulse-server serve --config /etc/platpulse/server.toml
```

`--web-assets /path/to/web` is an explicit `serve` override for tests,
development, and custom installations. It has higher precedence than the
`web_root` value in `server.toml`, which has higher precedence than the
built-in default.

### Storage Backend

The Server stores its data in a SQLite database file by default. The PostgreSQL
Storage Backend is chosen explicitly by name ([ADR 0013](../adr/0013-postgresql-server-storage-backend.md));
it is never inferred from the environment, a URL, or a DSN:

```toml
[storage]
backend = "postgres"

[storage.postgres]
host = "/run/postgresql"   # or a host name / IP address
port = 5432
database = "platpulse"
user = "platpulse"
password_file = "/etc/platpulse/secrets/postgres-password"
sslmode = "disable"        # accepted only over a Unix-domain socket
connect_timeout_secs = 10
ownership_heartbeat_secs = 5
```

Every setting comes from `[storage.postgres]`: the `PG*` libpq variables and
`.pgpass` are ignored, and a `password`, `url`, or `dsn` key is rejected. The
password is read only from `password_file`, which follows the same policy as
`pepper_file` (a same-user 0600 regular file, no symlinked path). It never
appears in a log, an error, or the redacted connection `Debug` output. When the
PostgreSQL backend is selected, `db_path` must be absent; leave `backup_dir`
in place only if the SQLite tooling still uses it.

`sslmode` defaults to `verify-full` and accepts only `verify-ca` as another
value; `disable` must be written out explicitly and is accepted only for a
Unix-domain socket directory, so leaving the key out never downgrades a socket
connection to plaintext by accident. A remote connection is always encrypted, so
`disable` over TCP is refused. `ssl_root_cert_file` supplies the CA certificate and cannot accompany
`disable`. Both timers are 1..=300 s and the heartbeat must be shorter than the
connect timeout.

The PostgreSQL baseline migration lives beside the SQLite ones in
`crates/platpulse-server/migrations-postgres` and is applied automatically after
ownership is taken. Both backends share one logical schema version and CI proves
their tables, columns, constraints, and indexes stay equivalent.

Provisioning the role and database, TLS and certificate trust, the password file,
the single-owner rule, the behaviour after an ownership loss and the operations
that are not available on PostgreSQL are covered in "PostgreSQL Storage
Backend" below. An existing SQLite deployment moves onto PostgreSQL only through
the offline Storage Backend Migration (`storage migrate`, one way); its runbook,
refusal conditions and abort rule are in "Upgrade and rollback".

The current production provider is Telegram. If no channel is configured,
Notification Events are still recorded but no Delivery rows are created; configure
a channel with a path to a secret file and a destination:

```toml
[notifications.telegram]
enabled = true
token_file = "/etc/platpulse/secrets/telegram-token"
chat_id = "123456789"
max_attempts = 5
retry_base_seconds = 60
```

The token file must be a regular file readable by the `platpulse-server` service
user (mode `0600` is recommended when its owner/group is that service account).
The provider reads and trims it for each send, so controlled token rotation takes
effect without a restart. For a queued Delivery, an unreadable or empty token file
or a disabled/missing channel produces `state = failed` with
`last_error_kind = config`, without calling Telegram or consuming an attempt; a
new Owner test-send action rejects a disabled channel before creating its Event or
Delivery. Provider/API transport failures retry with bounded exponential backoff;
exhaustion becomes `dead_letter`. An Owner can manually retry a `failed`,
`retry_scheduled`, or `dead_letter` Delivery after correcting the configuration.
Delivery destination is masked; the full token and directory path never enter
DTOs, Audit, or logs, while the Channel DTO intentionally exposes only the
secret-file basename as `providerRef`.
