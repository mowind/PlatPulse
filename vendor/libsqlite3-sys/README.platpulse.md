# Vendored libsqlite3-sys (PlatPulse)

This directory is the upstream `libsqlite3-sys` **0.30.1** crate with exactly three
files replaced by the **SQLite 3.53.2** amalgamation.

## Why the patch exists

`sqlx` 0.8.6 enables `sqlx-sqlite/bundled`, which resolves to
`libsqlite3-sys` `^0.30.1` (see `crates/platpulse-server/Cargo.toml:32` and
`crates/platpulse-agent/Cargo.toml:24`). That release embeds SQLite **3.46.0**,
which is inside the affected range of the upstream WAL-reset corruption bug
(SQLite < 3.51.3); the live Server database was corrupted in production on
2026-09-17 (see `docs/research/server-db-corruption-investigation.md` §7
recommendation 1).

Raising the bundled SQLite needs a `libsqlite3-sys` `>= 0.35`-class release,
which the `^0.30.1` requirement of sqlx-sqlite 0.8.6 forbids; only sqlx 0.9.0
relaxes it (`>= 0.30.1, < 0.38.0`). Upgrading sqlx is a separate, breaking
migration, so this patch pins the C library forward without moving sqlx.

## What changed

| Path | Change |
| --- | --- |
| `sqlite3/sqlite3.c` | replaced with the 3.53.2 amalgamation (sha256 `0a409f1633283fa31a9126b11fbfd64a1991c5d30defad07e5745d4667f5e23d`) |
| `sqlite3/sqlite3.h` | replaced with the 3.53.2 header (sha256 `9e69a1353a4288450b0d5239ede11fc7f1f4c8e5eb07491fc8317eacb5b7de7e`) |
| `sqlite3/sqlite3ext.h` | replaced with the 3.53.2 header |

Everything else - `build.rs`, `src/`, `Cargo.toml` (version stays `0.30.1` so the
`^0.30.1` requirement is satisfied), `wrapper.h`, `wrapper_ext.h`, `LICENSE`,
`README.md`, and the pre-generated `sqlite3/bindgen_bundled_version*.rs`
bindings - is upstream 0.30.1 unchanged. The bindings are kept because the C API
is append-only/ABI stable and 0.30.1's bindings carry no
`libsqlite3_sys_<version>` cfg gates; only the C source needed to move.
`sqlcipher/` and `bindgen-bindings/` were dropped: neither is read by a build
that compiles the plain bundled amalgamation.

Wiring: root `Cargo.toml` has

```toml
[patch.crates-io]
libsqlite3-sys = { path = "vendor/libsqlite3-sys" }
```

## Exit plan

Delete this directory and the `[patch.crates-io]` entry once the workspace moves
to a `sqlx` release that accepts `libsqlite3-sys >= 0.35` (sqlx 0.9.0 and later),
and re-verify with `strings target/release/platpulse-server | grep -E '^3\.[0-9]+\.[0-9]+$'`.
