//! Spike #253 (ADR 0013 Q13, #226 S2) — THROWAWAY, not for merge.
//!
//! Validates whether one `sqlx::Any` pool plus one `$N` SQL text can carry the
//! Server's Report Ingestion on both SQLite and PostgreSQL. Every test runs on
//! the real schema: SQLite through the Server migrations, PostgreSQL through
//! `tests/any_spike/pg_schema.sql` (a mechanical translation of the same final
//! schema, see `translate_schema.py`).
//!
//! PostgreSQL tests need `PLATPULSE_SPIKE_PG_URL` (an admin URL to a scratch
//! server, e.g. `postgres://spike:spike@127.0.0.1:55417/spike?sslmode=disable`);
//! each test creates and drops its own database. Without it only the SQLite
//! half runs. The benchmark is `#[ignore]`:
//!
//! ```text
//! PLATPULSE_SPIKE_PG_URL=... cargo test -p platpulse-server --release --test any_spike -- --ignored --nocapture
//! ```
#![allow(clippy::too_many_lines)]

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::error::ErrorKind;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{AnyConnection, AnyPool, Connection, Executor, FromRow, Row, SqliteConnection};
use tempfile::TempDir;

const PG_SCHEMA: &str = include_str!("any_spike/pg_schema.sql");
const NOW: &str = "2026-10-22T02:00:00Z";

// ---------------------------------------------------------------------------
// Backends
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Sqlite,
    Postgres,
}

struct Backend {
    kind: Kind,
    pool: AnyPool,
    _guard: Guard,
}

enum Guard {
    Sqlite(#[allow(dead_code)] TempDir, PathBuf),
    Postgres { admin_url: String, database: String },
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Guard::Postgres {
            admin_url,
            database,
        } = self
        {
            let admin_url = admin_url.clone();
            let database = database.clone();
            // Best effort cleanup on a fresh runtime thread (Drop is sync).
            let _ = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    if let Ok(mut admin) = AnyConnection::connect(&admin_url).await {
                        let _ = admin
                            .execute(
                                format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)").as_str(),
                            )
                            .await;
                    }
                });
            })
            .join();
        }
    }
}

fn install() {
    sqlx::any::install_default_drivers();
}

/// Required SQLite connection settings. `AnyConnectOptions` is URL-only and
/// `SqliteConnectOptions::from_url` understands just mode/cache/immutable/vfs,
/// so synchronous/busy_timeout must be re-applied per connection.
async fn sqlite_after_connect(conn: &mut AnyConnection, busy_ms: u64) -> Result<(), sqlx::Error> {
    if conn.backend_name() == "SQLite" {
        conn.execute("PRAGMA synchronous=FULL").await?;
        conn.execute(format!("PRAGMA busy_timeout={busy_ms}").as_str())
            .await?;
    }
    Ok(())
}

async fn migrated_sqlite_file(dir: &Path) -> PathBuf {
    let path = dir.join("server.db");
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool.close().await;
    path
}

fn sqlite_url(path: &Path) -> String {
    format!("sqlite://{}?mode=rw", path.display())
}

async fn any_sqlite_pool(path: &Path, busy_ms: u64) -> AnyPool {
    install();
    AnyPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _| {
            Box::pin(async move { sqlite_after_connect(conn, busy_ms).await })
        })
        .connect(&sqlite_url(path))
        .await
        .unwrap()
}

async fn sqlite_backend() -> Backend {
    let dir = TempDir::new().unwrap();
    let path = migrated_sqlite_file(dir.path()).await;
    let pool = any_sqlite_pool(&path, 5_000).await;
    Backend {
        kind: Kind::Sqlite,
        pool,
        _guard: Guard::Sqlite(dir, path),
    }
}

fn pg_admin_url() -> Option<String> {
    std::env::var("PLATPULSE_SPIKE_PG_URL").ok()
}

fn with_database(admin_url: &str, database: &str) -> String {
    let mut url = url::Url::parse(admin_url).unwrap();
    url.set_path(database);
    url.to_string()
}

async fn pg_backend_with(schema: Option<&str>) -> Option<Backend> {
    install();
    let admin_url = pg_admin_url()?;
    let database = format!("spike_{}", uuid::Uuid::new_v4().simple());
    let mut admin = AnyConnection::connect(&admin_url).await.unwrap();
    admin
        .execute(format!("CREATE DATABASE {database}").as_str())
        .await
        .unwrap();
    admin.close().await.unwrap();
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&with_database(&admin_url, &database))
        .await
        .unwrap();
    if let Some(schema) = schema {
        // Multi-statement text over the simple query protocol works through Any.
        sqlx::raw_sql(schema).execute(&pool).await.unwrap();
    }
    Some(Backend {
        kind: Kind::Postgres,
        pool,
        _guard: Guard::Postgres {
            admin_url,
            database,
        },
    })
}

async fn pg_backend() -> Option<Backend> {
    pg_backend_with(Some(PG_SCHEMA)).await
}

async fn backends() -> Vec<Backend> {
    let mut out = vec![sqlite_backend().await];
    match pg_backend().await {
        Some(pg) => out.push(pg),
        None => eprintln!("PLATPULSE_SPIKE_PG_URL unset: PostgreSQL half skipped"),
    }
    out
}

fn code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|e| e.code())
        .map(|c| c.into_owned())
}

// ---------------------------------------------------------------------------
// Seeds (real schema, real column lists)
// ---------------------------------------------------------------------------

async fn seed_agent(conn: &mut AnyConnection, agent_id: &str) {
    sqlx::query(
        "INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ($1, 0, $2, $3)",
    )
    .bind(agent_id)
    .bind(NOW)
    .bind(NOW)
    .execute(&mut *conn)
    .await
    .unwrap();
}

async fn seed_network(conn: &mut AnyConnection) {
    sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT DO NOTHING")
        .bind("platon-mainnet")
        .bind("PlatON Mainnet")
        .bind(format!("0x{}", "ab".repeat(32)))
        .bind(210_425_i64)
        .bind(100_i64)
        .bind("lat")
        .bind(NOW)
        .bind(NOW)
        .execute(&mut *conn)
        .await
        .unwrap();
}

async fn seed_node(conn: &mut AnyConnection, agent_id: &str, node_id: &str) {
    seed_network(conn).await;
    sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, 'platon-mainnet', 'http://127.0.0.1:6789', 'active', 1, $3, $4)")
        .bind(node_id)
        .bind(agent_id)
        .bind(NOW)
        .bind(NOW)
        .execute(&mut *conn)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// Q1 — boolean mapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn q1_boolean_mapping() {
    for backend in backends().await {
        let kind = backend.kind;
        let mut conn = backend.pool.acquire().await.unwrap();
        seed_agent(&mut conn, "agent-a").await;
        sqlx::query("UPDATE agents SET shutdown_forced = $1 WHERE agent_id = $2")
            .bind(1_i64)
            .bind("agent-a")
            .execute(&mut *conn)
            .await
            .unwrap();

        // A mirrored 0/1 integer flag decodes as i64 on both backends ...
        let as_int: i64 =
            sqlx::query_scalar("SELECT shutdown_forced FROM agents WHERE agent_id = $1")
                .bind("agent-a")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(as_int, 1, "{kind:?}");
        // ... but never as bool: Any's bool decode only accepts a Bool value.
        let as_bool =
            sqlx::query_scalar::<_, bool>("SELECT shutdown_forced FROM agents WHERE agent_id = $1")
                .bind("agent-a")
                .fetch_one(&mut *conn)
                .await;
        assert!(
            as_bool.is_err(),
            "{kind:?}: integer flag must not decode as bool"
        );
        eprintln!(
            "[q1 {kind:?}] integer flag as bool -> {}",
            as_bool.unwrap_err()
        );

        // Predicate expressions differ by backend: SQLite yields an integer,
        // PostgreSQL a boolean. A CASE wrapper yields an integer on both.
        let exists_bool = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE agent_id = $1)",
        )
        .bind("agent-a")
        .fetch_one(&mut *conn)
        .await;
        let exists_int =
            sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM agents WHERE agent_id = $1)")
                .bind("agent-a")
                .fetch_one(&mut *conn)
                .await;
        match kind {
            Kind::Sqlite => assert!(exists_bool.is_err() && exists_int.is_ok()),
            Kind::Postgres => assert!(exists_bool.is_ok() && exists_int.is_err()),
        }
        let wrapped: i64 = sqlx::query_scalar(
            "SELECT CASE WHEN EXISTS(SELECT 1 FROM agents WHERE agent_id = $1) THEN 1 ELSE 0 END",
        )
        .bind("agent-a")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(wrapped, 1, "{kind:?}");

        // Binding a Rust bool: SQLite stores 0/1, PostgreSQL rejects it.
        let bind_bool = sqlx::query("UPDATE agents SET shutdown_forced = $1 WHERE agent_id = $2")
            .bind(false)
            .bind("agent-a")
            .execute(&mut *conn)
            .await;
        match kind {
            Kind::Sqlite => assert!(bind_bool.is_ok()),
            Kind::Postgres => {
                // Not even a clean datatype error: the bool is sent in binary
                // for an int8 parameter and the server rejects the Bind
                // message (08P01 protocol_violation).
                let error = bind_bool.unwrap_err();
                eprintln!(
                    "[q1 Postgres] bind bool into BIGINT -> {:?} {error}",
                    code(&error)
                );
                assert_eq!(code(&error).as_deref(), Some("08P01"));
            }
        }

        // `flag = 1` stays portable on mirrored integers (an aggregate over
        // them needs a CAST on PostgreSQL, see portability_traps).
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agents WHERE shutdown_forced = 1 OR shutdown_forced = 0",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(count, 1, "{kind:?}");

        // The alternative — BOOLEAN on both sides. SQLite stores a BOOLEAN
        // column as an integer, and the Any driver refuses its declared type.
        if kind == Kind::Sqlite {
            conn.execute("CREATE TABLE flag_probe (flag BOOLEAN NOT NULL)")
                .await
                .unwrap();
            conn.execute("INSERT INTO flag_probe VALUES (1)")
                .await
                .unwrap();
            let as_bool = sqlx::query_scalar::<_, bool>("SELECT flag FROM flag_probe")
                .fetch_one(&mut *conn)
                .await;
            let as_int = sqlx::query_scalar::<_, i64>("SELECT flag FROM flag_probe")
                .fetch_one(&mut *conn)
                .await;
            eprintln!(
                "[q1 Sqlite] BOOLEAN column -> as bool: {:?}, as i64: {:?}",
                as_bool.as_ref().map_err(ToString::to_string),
                as_int.as_ref().map_err(ToString::to_string)
            );
            assert!(
                as_bool.is_err(),
                "SQLite BOOLEAN never decodes as bool through Any"
            );
        }
        // ... and on PostgreSQL integer literals stop comparing (42883).
        if kind == Kind::Postgres {
            conn.execute("CREATE TABLE flag_probe (flag BOOLEAN NOT NULL)")
                .await
                .unwrap();
            conn.execute("INSERT INTO flag_probe VALUES (TRUE)")
                .await
                .unwrap();
            let int_literal =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM flag_probe WHERE flag = 1")
                    .fetch_one(&mut *conn)
                    .await;
            assert_eq!(code(&int_literal.unwrap_err()).as_deref(), Some("42883"));
            let as_int = sqlx::query_scalar::<_, i64>("SELECT flag FROM flag_probe")
                .fetch_one(&mut *conn)
                .await;
            assert!(
                as_int.is_err(),
                "BOOLEAN does not decode as i64 through Any"
            );
            let as_bool: bool = sqlx::query_scalar("SELECT flag FROM flag_probe")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert!(as_bool);
        }
    }
}

// ---------------------------------------------------------------------------
// Q2 — BLOB / bytea round trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn q2_blob_round_trip() {
    for backend in backends().await {
        let kind = backend.kind;
        let mut conn = backend.pool.acquire().await.unwrap();
        seed_agent(&mut conn, "agent-a").await;
        // Credential digest with NUL and 0xff bytes, looked up by equality.
        let digest: Vec<u8> = (0..32_u8)
            .map(|i| i.wrapping_mul(37) ^ 0xa5)
            .chain([0, 255])
            .collect();
        sqlx::query("INSERT INTO agent_credentials (credential_id, agent_id, credential_digest, created_at) VALUES ($1, $2, $3, $4)")
            .bind("cred-1")
            .bind("agent-a")
            .bind(&digest)
            .bind(NOW)
            .execute(&mut *conn)
            .await
            .unwrap();
        let found: String = sqlx::query_scalar(
            "SELECT agent_id FROM agent_credentials WHERE credential_digest = $1 AND revoked_at IS NULL",
        )
        .bind(digest.as_slice())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(found, "agent-a", "{kind:?}");
        let duplicate = sqlx::query("INSERT INTO agent_credentials (credential_id, agent_id, credential_digest, created_at) VALUES ($1, $2, $3, $4)")
            .bind("cred-2")
            .bind("agent-a")
            .bind(&digest)
            .bind(NOW)
            .execute(&mut *conn)
            .await
            .unwrap_err();
        assert_eq!(
            duplicate.as_database_error().unwrap().kind(),
            ErrorKind::UniqueViolation,
            "{kind:?}"
        );

        // Receipt body: every byte value, larger than one page / TOAST threshold.
        let body: Vec<u8> = (0..300_000_u32).map(|i| (i % 256) as u8).collect();
        sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
            .bind("report-1").bind("agent-a").bind(0_i64).bind("boot-1").bind(1_i64)
            .bind("0xsha").bind("accepted").bind(&body).bind(NOW)
            .execute(&mut *conn)
            .await
            .unwrap();
        let stored: Vec<u8> = sqlx::query_scalar(
            "SELECT receipt_body FROM agent_report_receipts WHERE report_id = $1",
        )
        .bind("report-1")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(stored, body, "{kind:?}");
        // length() is bytes for BLOB on SQLite and for bytea on PostgreSQL.
        let len: i64 = sqlx::query_scalar(
            "SELECT length(receipt_body) FROM agent_report_receipts WHERE report_id = $1",
        )
        .bind("report-1")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(len, 300_000, "{kind:?}");
    }
}

// ---------------------------------------------------------------------------
// Q3/Q4 — rows_affected under ON CONFLICT, RETURNING
// ---------------------------------------------------------------------------

#[tokio::test]
async fn q3_q4_rows_affected_and_returning() {
    for backend in backends().await {
        let kind = backend.kind;
        let mut conn = backend.pool.acquire().await.unwrap();
        seed_agent(&mut conn, "agent-a").await;
        seed_node(&mut conn, "agent-a", "node-1").await;
        let gap = "INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, $2, $3, $4, $5, $6, 'agent') ON CONFLICT DO NOTHING";
        let first = sqlx::query(gap)
            .bind("node-1")
            .bind(10_i64)
            .bind(20_i64)
            .bind("history_gap")
            .bind("r")
            .bind(NOW)
            .execute(&mut *conn)
            .await
            .unwrap();
        let second = sqlx::query(gap)
            .bind("node-1")
            .bind(10_i64)
            .bind(20_i64)
            .bind("history_gap")
            .bind("r")
            .bind(NOW)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            (first.rows_affected(), second.rows_affected()),
            (1, 0),
            "{kind:?}"
        );

        let upsert = "INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, $2, $3, 'server_rejected', $4, $5, 'server') ON CONFLICT(node_id, from_height, to_height, kind) DO UPDATE SET reason=excluded.reason, created_at=excluded.created_at, authored_by=excluded.authored_by";
        let a = sqlx::query(upsert)
            .bind("node-1")
            .bind(1_i64)
            .bind(2_i64)
            .bind("x")
            .bind(NOW)
            .execute(&mut *conn)
            .await
            .unwrap();
        let b = sqlx::query(upsert)
            .bind("node-1")
            .bind(1_i64)
            .bind(2_i64)
            .bind("y")
            .bind(NOW)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!((a.rows_affected(), b.rows_affected()), (1, 1), "{kind:?}");

        // DO UPDATE ... WHERE false counts 0 on both.
        let guarded = sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 1, 2, 'server_rejected', 'z', $2, 'server') ON CONFLICT(node_id, from_height, to_height, kind) DO UPDATE SET reason=excluded.reason WHERE block_history_gaps.reason = 'never'")
            .bind("node-1").bind(NOW).execute(&mut *conn).await.unwrap();
        assert_eq!(guarded.rows_affected(), 0, "{kind:?}");

        // UPDATE to identical values counts matched rows on both.
        let same = sqlx::query("UPDATE agents SET agent_epoch = agent_epoch WHERE agent_id = $1")
            .bind("agent-a")
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(same.rows_affected(), 1, "{kind:?}");

        // RETURNING from the identity/rowid key (insert_audit_change SQL).
        let audit = "INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING audit_event_id";
        let mut ids = Vec::new();
        for _ in 0..2 {
            let id: i64 = sqlx::query_scalar(audit)
                .bind(None::<String>)
                .bind("spike")
                .bind("agent")
                .bind("agent-a")
                .bind(None::<String>)
                .bind(Some("{}".to_owned()))
                .bind(NOW)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            ids.push(id);
        }
        assert_eq!(ids[1], ids[0] + 1, "{kind:?}");

        // ON CONFLICT DO NOTHING RETURNING yields no row on conflict.
        let returning_gap = "INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 10, 20, 'history_gap', 'r', $2, 'agent') ON CONFLICT DO NOTHING RETURNING gap_id";
        let none: Option<i64> = sqlx::query_scalar(returning_gap)
            .bind("node-1")
            .bind(NOW)
            .fetch_optional(&mut *conn)
            .await
            .unwrap();
        assert_eq!(none, None, "{kind:?}");
    }
}

// ---------------------------------------------------------------------------
// Q5 — error classification through DatabaseError::code()
// ---------------------------------------------------------------------------

/// The Server's current classifier (`http::health::is_corruption_error`).
fn current_is_corruption_error(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .and_then(|code| code.parse::<i32>().ok())
        .is_some_and(|code| matches!(code & 0xff, 11 | 26))
}

#[tokio::test]
async fn q5_error_classification_sqlite() {
    let backend = sqlite_backend().await;
    let Guard::Sqlite(_, path) = &backend._guard else {
        unreachable!()
    };
    let mut conn = backend.pool.acquire().await.unwrap();
    seed_agent(&mut conn, "agent-a").await;

    // Unique / PK / FK violations: native SQLite extended codes pass through
    // Any, kind() is populated, and the concrete type can be downcast.
    let pk = sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-a', 0, $1, $2)")
        .bind(NOW).bind(NOW).execute(&mut *conn).await.unwrap_err();
    let db = pk.as_database_error().unwrap();
    assert_eq!(db.code().as_deref(), Some("1555"));
    assert_eq!(db.kind(), ErrorKind::UniqueViolation);
    assert!(db.try_downcast_ref::<sqlx::sqlite::SqliteError>().is_some());
    let fk = sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, inventory_revision, first_seen_at, updated_at) VALUES ('n', 'missing', 'missing', 'x', 'active', 1, $1, $2)")
        .bind(NOW).bind(NOW).execute(&mut *conn).await.unwrap_err();
    assert_eq!(code(&fk).as_deref(), Some("787"));
    assert_eq!(
        fk.as_database_error().unwrap().kind(),
        ErrorKind::ForeignKeyViolation
    );
    drop(conn);

    // BUSY: another connection holds the write lock past busy_timeout.
    let mut holder = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
        .await
        .unwrap();
    holder.execute("BEGIN IMMEDIATE").await.unwrap();
    let impatient = any_sqlite_pool(path, 50).await;
    let busy = sqlx::query("UPDATE agents SET updated_at = $1")
        .bind(NOW)
        .execute(&impatient)
        .await
        .unwrap_err();
    assert_eq!(
        code(&busy).map(|c| c.parse::<i32>().unwrap() & 0xff),
        Some(5),
        "{busy}"
    );
    holder.execute("ROLLBACK").await.unwrap();

    // NOTADB: not a database file at all.
    let dir = TempDir::new().unwrap();
    let garbage = dir.path().join("garbage.db");
    std::fs::write(&garbage, vec![b'x'; 8192]).unwrap();
    install();
    let notadb = match AnyPoolOptions::new()
        .max_connections(1)
        .connect(&sqlite_url(&garbage))
        .await
    {
        Ok(pool) => sqlx::query("SELECT count(*) FROM sqlite_master")
            .fetch_one(&pool)
            .await
            .map(|_| ())
            .unwrap_err(),
        Err(error) => error,
    };
    assert_eq!(code(&notadb).as_deref(), Some("26"), "{notadb}");
    assert!(current_is_corruption_error(&notadb));

    // CORRUPT: smash a table b-tree page.
    let corrupt_path = dir.path().join("corrupt.db");
    {
        let mut c = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&corrupt_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        c.execute("CREATE TABLE t (v TEXT)").await.unwrap();
        for i in 0..2000 {
            sqlx::query("INSERT INTO t VALUES ($1)")
                .bind(format!("row-{i:06}-{}", "p".repeat(40)))
                .execute(&mut c)
                .await
                .unwrap();
        }
        c.close().await.unwrap();
    }
    let mut bytes = std::fs::read(&corrupt_path).unwrap();
    for b in &mut bytes[4096 * 2..4096 * 6] {
        *b = 0xff;
    }
    std::fs::write(&corrupt_path, bytes).unwrap();
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&sqlite_url(&corrupt_path))
        .await
        .unwrap();
    let corrupt = sqlx::query("SELECT count(*), max(v) FROM t")
        .fetch_one(&pool)
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(
        code(&corrupt).map(|c| c.parse::<i32>().unwrap() & 0xff),
        Some(11),
        "{corrupt}"
    );
    assert!(current_is_corruption_error(&corrupt));
}

#[tokio::test]
async fn q5_error_classification_postgres() {
    let Some(backend) = pg_backend().await else {
        return;
    };
    let mut conn = backend.pool.acquire().await.unwrap();
    seed_agent(&mut conn, "agent-a").await;
    let pk = sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-a', 0, $1, $2)")
        .bind(NOW).bind(NOW).execute(&mut *conn).await.unwrap_err();
    let db = pk.as_database_error().unwrap();
    assert_eq!(db.code().as_deref(), Some("23505"));
    assert_eq!(db.kind(), ErrorKind::UniqueViolation);
    let pg = db
        .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
        .unwrap();
    assert_eq!(pg.constraint(), Some("agents_pkey"));
    let fk = sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, inventory_revision, first_seen_at, updated_at) VALUES ('n', 'missing', 'missing', 'x', 'active', 1, $1, $2)")
        .bind(NOW).bind(NOW).execute(&mut *conn).await.unwrap_err();
    assert_eq!(code(&fk).as_deref(), Some("23503"));
    assert_eq!(
        fk.as_database_error().unwrap().kind(),
        ErrorKind::ForeignKeyViolation
    );

    // Corruption SQLSTATEs cannot be produced on a healthy cluster; raising
    // them proves the class reaches code() unchanged through Any.
    for sqlstate in ["XX001", "XX002"] {
        let raised = sqlx::raw_sql(&format!(
            "DO $$ BEGIN RAISE EXCEPTION 'simulated' USING ERRCODE = '{sqlstate}'; END $$"
        ))
        .execute(&mut *conn)
        .await
        .unwrap_err();
        assert_eq!(code(&raised).as_deref(), Some(sqlstate));
        // Not numeric, so the current SQLite classifier misses them.
        assert!(!current_is_corruption_error(&raised));
    }
    // An all-digit SQLSTATE whose low byte is 11 (22027 trim_error) is
    // misclassified as SQLITE_CORRUPT by the current numeric classifier:
    // classification must dispatch on the backend, not parse codes blindly.
    let trim = sqlx::query_scalar::<_, String>("SELECT trim(BOTH '' FROM $1::bytea)::text")
        .bind("x")
        .fetch_one(&mut *conn)
        .await;
    let raised = match trim {
        Err(e) if code(&e).as_deref() == Some("22027") => e,
        _ => sqlx::raw_sql("DO $$ BEGIN RAISE EXCEPTION 'trim' USING ERRCODE = '22027'; END $$")
            .execute(&mut *conn)
            .await
            .unwrap_err(),
    };
    assert_eq!(code(&raised).as_deref(), Some("22027"));
    assert!(current_is_corruption_error(&raised), "22027 & 0xff == 11");
}

// ---------------------------------------------------------------------------
// Q7 — FromRow, Transaction<Any>, savepoints, failed-statement semantics,
//      defer_foreign_keys replacement
// ---------------------------------------------------------------------------

/// Same shape as `http::report_ingestion::AgentRow`.
#[derive(Debug, FromRow, PartialEq)]
struct AgentRow {
    agent_epoch: i64,
    active_boot_id: Option<String>,
    active_boot_status: String,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    last_report_sequence: Option<i64>,
    last_inventory_revision: i64,
}

#[tokio::test]
async fn q7_from_row_and_transactions() {
    for backend in backends().await {
        let kind = backend.kind;
        let mut conn = backend.pool.acquire().await.unwrap();
        seed_agent(&mut conn, "agent-a").await;
        let row = sqlx::query_as::<_, AgentRow>("SELECT agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, last_report_sequence, last_inventory_revision FROM agents WHERE agent_id = $1 AND deleted_at IS NULL")
            .bind("agent-a")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(row.active_boot_status, "active", "{kind:?}");
        assert_eq!(row.last_report_sequence, None, "{kind:?}");

        let audit = "INSERT INTO audit_events (event_kind, target_kind, created_at) VALUES ($1, 'spike', $2)";
        let count = "SELECT COUNT(*) FROM audit_events WHERE event_kind = $1";

        // Rollback and commit.
        let mut tx = conn.begin().await.unwrap();
        sqlx::query(audit)
            .bind("rolled-back")
            .bind(NOW)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        sqlx::query(audit)
            .bind("committed")
            .bind(NOW)
            .execute(&mut *tx)
            .await
            .unwrap();
        // Nested begin is a SAVEPOINT on both backends.
        let mut sp = tx.begin().await.unwrap();
        sqlx::query(audit)
            .bind("savepoint")
            .bind(NOW)
            .execute(&mut *sp)
            .await
            .unwrap();
        sp.rollback().await.unwrap();
        tx.commit().await.unwrap();
        for (event, expected) in [("rolled-back", 0_i64), ("committed", 1), ("savepoint", 0)] {
            let n: i64 = sqlx::query_scalar(count)
                .bind(event)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert_eq!(n, expected, "{kind:?} {event}");
        }

        // A failed statement whose error is ignored (the `let _ = ...` /
        // `.ok()` pattern inside the Ingestion transaction). SQLite keeps the
        // transaction usable; PostgreSQL aborts it (25P02) and COMMIT then
        // silently becomes ROLLBACK — sqlx reports success.
        let mut tx = conn.begin().await.unwrap();
        sqlx::query(audit)
            .bind("before-failure")
            .bind(NOW)
            .execute(&mut *tx)
            .await
            .unwrap();
        let _ignored = sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-a', 0, $1, $2)")
            .bind(NOW).bind(NOW).execute(&mut *tx).await;
        let next = sqlx::query(audit)
            .bind("after-failure")
            .bind(NOW)
            .execute(&mut *tx)
            .await;
        let commit = tx.commit().await;
        let persisted: i64 = sqlx::query_scalar(count)
            .bind("before-failure")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        eprintln!(
            "[q7 {kind:?}] after ignored failure: next={:?} commit={:?} persisted={persisted}",
            next.as_ref().map(|_| ()).map_err(code),
            commit.as_ref().map(|_| ())
        );
        match kind {
            Kind::Sqlite => {
                assert!(next.is_ok());
                assert!(commit.is_ok());
                assert_eq!(persisted, 1);
            }
            Kind::Postgres => {
                assert_eq!(code(&next.unwrap_err()).as_deref(), Some("25P02"));
                assert!(commit.is_ok(), "sqlx does not surface COMMIT->ROLLBACK");
                assert_eq!(persisted, 0, "writes before the ignored failure are lost");
            }
        }

        // Mitigation: run tolerated statements inside a savepoint.
        let mut tx = conn.begin().await.unwrap();
        sqlx::query(audit)
            .bind("guarded")
            .bind(NOW)
            .execute(&mut *tx)
            .await
            .unwrap();
        {
            let mut sp = tx.begin().await.unwrap();
            let failed = sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-a', 0, $1, $2)")
                .bind(NOW).bind(NOW).execute(&mut *sp).await;
            assert!(failed.is_err());
            sp.rollback().await.unwrap();
        }
        sqlx::query(audit)
            .bind("guarded")
            .bind(NOW)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let guarded: i64 = sqlx::query_scalar(count)
            .bind("guarded")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(guarded, 2, "{kind:?}");
    }
}

/// Backend-dispatched replacement for `PRAGMA defer_foreign_keys=ON`.
/// PostgreSQL needs the FK declared DEFERRABLE (INITIALLY IMMEDIATE keeps
/// every other statement's semantics) and `SET CONSTRAINTS` per transaction.
async fn defer_node_ownership_fk(conn: &mut AnyConnection) -> Result<(), sqlx::Error> {
    match conn.backend_name() {
        "SQLite" => conn
            .execute("PRAGMA defer_foreign_keys=ON")
            .await
            .map(|_| ()),
        "PostgreSQL" => conn
            .execute("SET CONSTRAINTS component_status_agent_id_node_id_fkey DEFERRED")
            .await
            .map(|_| ()),
        other => panic!("unexpected backend {other}"),
    }
}

async fn transfer_node(conn: &mut AnyConnection, defer: bool) -> Result<(), sqlx::Error> {
    let mut tx = conn.begin().await?;
    if defer {
        defer_node_ownership_fk(&mut tx).await?;
    }
    sqlx::query("UPDATE nodes SET agent_id=$1, updated_at=$2 WHERE node_id=$3")
        .bind("agent-b")
        .bind(NOW)
        .bind("node-1")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE component_status SET agent_id=$1 WHERE node_id=$2")
        .bind("agent-b")
        .bind("node-1")
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

#[tokio::test]
async fn q7_defer_foreign_keys_replacement() {
    for backend in backends().await {
        let kind = backend.kind;
        let mut conn = backend.pool.acquire().await.unwrap();
        seed_agent(&mut conn, "agent-a").await;
        seed_agent(&mut conn, "agent-b").await;
        seed_node(&mut conn, "agent-a", "node-1").await;
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, state_revision, value_revision) VALUES ('agent-a', 'node', 'node-1', 'node-1', 'chain.sync', 'ok', 1, 1)")
            .execute(&mut *conn).await.unwrap();

        let immediate = transfer_node(&mut conn, false).await.unwrap_err();
        assert_eq!(
            immediate.as_database_error().unwrap().kind(),
            ErrorKind::ForeignKeyViolation,
            "{kind:?}"
        );

        if kind == Kind::Postgres {
            // The translated schema keeps PostgreSQL's NOT DEFERRABLE default:
            // SET CONSTRAINTS refuses it (42809) instead of silently ignoring.
            let refused = transfer_node(&mut conn, true).await.unwrap_err();
            assert_eq!(code(&refused).as_deref(), Some("42809"), "{refused}");
            // A literal PRAGMA is a syntax error on PostgreSQL (and would abort
            // the Ingestion transaction).
            let pragma = conn
                .execute("PRAGMA defer_foreign_keys=ON")
                .await
                .unwrap_err();
            assert_eq!(code(&pragma).as_deref(), Some("42601"));
            conn.execute("ALTER TABLE component_status ALTER CONSTRAINT component_status_agent_id_node_id_fkey DEFERRABLE INITIALLY IMMEDIATE")
                .await
                .unwrap();
        }
        transfer_node(&mut conn, true).await.unwrap();
        let owner: String =
            sqlx::query_scalar("SELECT agent_id FROM component_status WHERE node_id = 'node-1'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(owner, "agent-b", "{kind:?}");
    }
}

// ---------------------------------------------------------------------------
// Other portability traps found while porting (feeds S3/S4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn portability_traps() {
    for backend in backends().await {
        let kind = backend.kind;
        let mut conn = backend.pool.acquire().await.unwrap();
        seed_agent(&mut conn, "agent-a").await;
        seed_node(&mut conn, "agent-a", "node-1").await;
        let pg = kind == Kind::Postgres;

        // SUM(bigint) is NUMERIC on PostgreSQL, which Any cannot decode.
        let sum = sqlx::query_scalar::<_, i64>("SELECT SUM(agent_epoch) FROM agents")
            .fetch_one(&mut *conn)
            .await;
        assert_eq!(sum.is_err(), pg, "{kind:?} SUM");
        if let Err(e) = &sum {
            eprintln!("[traps {kind:?}] SUM(bigint) -> {e}");
        }
        let cast: i64 =
            sqlx::query_scalar("SELECT CAST(COALESCE(SUM(agent_epoch), 0) AS BIGINT) FROM agents")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(cast, 0);
        // AVG is NUMERIC for integer input on PostgreSQL; CAST to DOUBLE PRECISION.
        let avg = sqlx::query_scalar::<_, f64>("SELECT AVG(agent_epoch) FROM agents")
            .fetch_one(&mut *conn)
            .await;
        assert_eq!(avg.is_err(), pg, "{kind:?} AVG");
        let avg: f64 =
            sqlx::query_scalar("SELECT CAST(AVG(agent_epoch) AS DOUBLE PRECISION) FROM agents")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(avg, 0.0);
        // Integer literals are int4 on PostgreSQL; Any widens them to i64.
        let one: i64 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(one, 1);

        // SQLite-only syntax / functions still present in domain SQL.
        let indexed_by = conn.execute("SELECT 1 FROM block_summaries INDEXED BY block_summaries_observed_idx WHERE node_id = 'x'").await;
        assert_eq!(indexed_by.is_err(), pg, "{kind:?} INDEXED BY");
        let json_len = sqlx::query_scalar::<_, i64>("SELECT json_array_length($1)")
            .bind("[1,2]")
            .fetch_one(&mut *conn)
            .await;
        assert_eq!(json_len.is_err(), pg, "{kind:?} json_array_length(text)");
        let ifnull = conn.execute("SELECT IFNULL(NULL, 1)").await;
        assert_eq!(ifnull.is_err(), pg, "{kind:?} IFNULL");

        // LIKE is ASCII case-insensitive on SQLite, case-sensitive on PostgreSQL.
        let like: i64 = sqlx::query_scalar("SELECT CASE WHEN 'ABC' LIKE 'abc' THEN 1 ELSE 0 END")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(like, if pg { 0 } else { 1 }, "{kind:?} LIKE");

        // PostgreSQL text cannot hold NUL (22021); SQLite stores it.
        let nul = sqlx::query("UPDATE agents SET display_name = $1 WHERE agent_id = 'agent-a'")
            .bind("bad\u{0}name")
            .execute(&mut *conn)
            .await;
        match kind {
            Kind::Sqlite => assert!(nul.is_ok()),
            Kind::Postgres => assert_eq!(code(&nul.unwrap_err()).as_deref(), Some("22021")),
        }

        // Column text ordering is byte-wise on both: SQLite BINARY, and the
        // translated PostgreSQL columns carry COLLATE "C".
        for id in ["b", "B", "a", "_"] {
            seed_agent(&mut conn, id).await;
        }
        let ordered: Vec<String> = sqlx::query_scalar(
            "SELECT agent_id FROM agents WHERE length(agent_id) = 1 ORDER BY agent_id",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(ordered, ["B", "_", "a", "b"], "{kind:?}");
        // ... but an explicit COLLATE "C" in SQL text is not portable: SQLite
        // has no collation named "C". Expressions (literals, concatenations)
        // on PostgreSQL use the database default collation instead.
        let explicit = sqlx::query_scalar::<_, String>(
            "SELECT agent_id FROM agents ORDER BY agent_id COLLATE \"C\" LIMIT 1",
        )
        .fetch_one(&mut *conn)
        .await;
        assert_eq!(explicit.is_err(), !pg, "{kind:?} COLLATE \"C\"");
        let literal: Vec<String> = sqlx::query_scalar("SELECT v FROM (SELECT 'b' AS v UNION ALL SELECT 'B' UNION ALL SELECT 'a' UNION ALL SELECT '_') AS t ORDER BY v")
            .fetch_all(&mut *conn).await.unwrap();
        eprintln!("[traps {kind:?}] literal ORDER BY (database default collation) -> {literal:?}");
    }
}

// ---------------------------------------------------------------------------
// Options: AnyConnectOptions is URL-only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn options_sqlite_pragmas_through_any() {
    let backend = sqlite_backend().await;
    let mut conn = backend.pool.acquire().await.unwrap();
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let sync: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    // foreign_keys comes from SqliteConnectOptions defaults, journal_mode is
    // persistent from the migration pool; synchronous/busy_timeout come only
    // from the after_connect hook.
    assert_eq!((fk, journal.as_str(), sync, busy), (1, "wal", 2, 5_000));
}

/// Run with e.g. `PGAPPNAME=leaked PGOPTIONS='-c statement_timeout=4321'`.
/// `PgConnectOptions::parse_from_url` starts from `new_without_pgpass()`, which
/// reads PG* environment variables; a URL can override host/port/user/
/// password/dbname/sslmode/sslrootcert but cannot *unset* PGOPTIONS,
/// PGSSLCERT, PGSSLKEY or PGAPPNAME. `.pgpass` is consulted when the URL
/// carries no password.
#[tokio::test]
#[ignore = "needs PG* variables set by the runner"]
async fn options_pg_environment_leaks_through_any_url() {
    let Some(backend) = pg_backend_with(None).await else {
        return;
    };
    let mut conn = backend.pool.acquire().await.unwrap();
    let app: String = sqlx::query_scalar("SELECT current_setting('application_name')")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let timeout: String = sqlx::query_scalar("SELECT current_setting('statement_timeout')")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    eprintln!("[options] application_name={app:?} statement_timeout={timeout:?}");
    if let Ok(expected) = std::env::var("PGAPPNAME") {
        assert_eq!(app, expected);
    }
    if std::env::var("PGOPTIONS").is_ok() {
        assert_eq!(timeout, "4321ms");
    }
    // The connect options keep the password in the URL; Debug must not print it.
    let url = with_database(&pg_admin_url().unwrap(), "x");
    let options = AnyConnectOptions::from_str(&url).unwrap();
    let debug = format!("{options:?}");
    let password = url::Url::parse(&url).unwrap().password().map(str::to_owned);
    let leaked = password.is_some_and(|p| debug.contains(&format!(":{p}@")));
    eprintln!("[options] AnyConnectOptions Debug contains the password: {leaked}");
    assert!(!leaked, "AnyConnectOptions Debug printed the password");
}

// ---------------------------------------------------------------------------
// Report Ingestion slice: verbatim SQL from http/report_ingestion.rs for the
// accepted path (receipt dedup, Agent/Boot checks, Inventory node upsert,
// component status, block summaries with high-water dedup, receipt BLOB,
// boot/agent state, audit RETURNING). Generated for both connection types so
// the benchmark compares the same code on native SQLite and on Any.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct SliceReport {
    agent_id: String,
    report_id: String,
    boot_id: String,
    sequence: i64,
    body_sha256: String,
    node_id: String,
    inventory_sha256: String,
    blocks: Vec<i64>,
    now: String,
}

#[derive(Debug, PartialEq, Eq)]
enum SliceOutcome {
    Accepted {
        receipt: Vec<u8>,
        new_blocks: u64,
        audit_event_id: i64,
    },
    Replayed {
        receipt: Vec<u8>,
    },
    Conflict,
}

#[derive(FromRow)]
struct ReceiptRow {
    report_body_sha256: String,
    receipt_body: Vec<u8>,
}

fn block_hash(node: &str, height: i64) -> String {
    use sha2::Digest;
    format!("0x{:x}", sha2::Sha256::digest(format!("{node}/{height}")))
}

fn receipt_bytes(report: &SliceReport, new_blocks: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "report_id": report.report_id,
        "disposition": "accepted",
        "inventory": {"revision": 1, "sha256": report.inventory_sha256},
        "nodes": [{"node_id": report.node_id, "accepted_blocks": new_blocks}],
    }))
    .unwrap()
}

macro_rules! define_slice {
    ($name:ident, $conn:ty) => {
        async fn $name(conn: &mut $conn, r: &SliceReport) -> Result<SliceOutcome, sqlx::Error> {
            let mut tx = conn.begin().await?;
            if let Some(existing) = sqlx::query_as::<_, ReceiptRow>(
                "SELECT report_body_sha256, receipt_body FROM agent_report_receipts WHERE report_id = $1",
            )
            .bind(&r.report_id)
            .fetch_optional(&mut *tx)
            .await?
            {
                if existing.report_body_sha256 != r.body_sha256 {
                    return Ok(SliceOutcome::Conflict);
                }
                sqlx::query_scalar::<_, i64>("SELECT 1 FROM agents WHERE agent_id = $1 AND deleted_at IS NULL")
                    .bind(&r.agent_id)
                    .fetch_one(&mut *tx)
                    .await?;
                return Ok(SliceOutcome::Replayed { receipt: existing.receipt_body });
            }
            let agent = sqlx::query_as::<_, AgentRow>(
                "SELECT agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, last_report_sequence, last_inventory_revision FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
            )
            .bind(&r.agent_id)
            .fetch_one(&mut *tx)
            .await?;
            let _old_boot: Option<String> = sqlx::query_scalar(
                "SELECT status FROM agent_boots WHERE agent_id=$1 AND agent_epoch=$2 AND boot_id=$3",
            )
            .bind(&r.agent_id).bind(agent.agent_epoch).bind(&r.boot_id)
            .fetch_optional(&mut *tx).await?;
            let prior_inventory: Option<Option<String>> = sqlx::query_scalar("SELECT inventory_sha256 FROM agents WHERE agent_id=$1")
                .bind(&r.agent_id).fetch_optional(&mut *tx).await?;
            let unchanged = prior_inventory.flatten().as_deref() == Some(r.inventory_sha256.as_str());
            let revision = if unchanged { agent.last_inventory_revision.max(1) } else { agent.last_inventory_revision + 1 };
            let sequence_conflict: Option<String> = sqlx::query_scalar("SELECT report_id FROM agent_report_receipts WHERE agent_id = $1 AND agent_epoch = $2 AND boot_id = $3 AND report_sequence = $4")
                .bind(&r.agent_id).bind(agent.agent_epoch).bind(&r.boot_id).bind(r.sequence)
                .fetch_optional(&mut *tx).await?;
            if sequence_conflict.is_some() {
                return Ok(SliceOutcome::Conflict);
            }
            let _known: Option<String> = sqlx::query_scalar("SELECT network_key FROM networks WHERE network_key = $1")
                .bind("platon-mainnet").fetch_optional(&mut *tx).await?;
            let _owner: Option<String> = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id = $1")
                .bind(&r.node_id).fetch_optional(&mut *tx).await?;
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, $3, $4, $5, 'active', 'private', $6, $7, $8) ON CONFLICT(node_id) DO UPDATE SET network_key=excluded.network_key, display_name=COALESCE(nodes.display_name, excluded.display_name), rpc_endpoint=excluded.rpc_endpoint, lifecycle='active', inventory_revision=excluded.inventory_revision, updated_at=excluded.updated_at")
                .bind(&r.node_id).bind(&r.agent_id).bind("platon-mainnet").bind(None::<String>)
                .bind("http://127.0.0.1:6789").bind(revision).bind(&r.now).bind(&r.now)
                .execute(&mut *tx).await?;
            sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, state_revision, value_revision, error_code, error_message) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) ON CONFLICT(agent_id, scope, scope_key, component_key) DO UPDATE SET state=excluded.state, attempted_at=excluded.attempted_at, observed_at=COALESCE(excluded.observed_at, component_status.observed_at), received_at=excluded.received_at, value_received_at=COALESCE(excluded.value_received_at, component_status.value_received_at), state_revision=excluded.state_revision, value_revision=CASE WHEN excluded.value_revision > 0 THEN excluded.value_revision ELSE component_status.value_revision END, error_code=excluded.error_code, error_message=excluded.error_message")
                .bind(&r.agent_id).bind("node").bind(&r.node_id).bind(&r.node_id).bind("chain.block")
                .bind("ok").bind(&r.now).bind(&r.now).bind(&r.now).bind(&r.now)
                .bind(r.sequence).bind(r.sequence).bind(None::<String>).bind(None::<String>)
                .execute(&mut *tx).await?;
            let head = r.blocks.iter().copied().max();
            sqlx::query("INSERT INTO block_history_state (node_id, current_head, resync_state, resync_last_progress_at, updated_at) VALUES ($1, $2, 'normal', $3, $4) ON CONFLICT(node_id) DO UPDATE SET current_head=excluded.current_head, resync_last_progress_at=CASE WHEN excluded.current_head IS NOT NULL AND (block_history_state.current_head IS NULL OR excluded.current_head > block_history_state.current_head) THEN excluded.resync_last_progress_at ELSE block_history_state.resync_last_progress_at END, updated_at=excluded.updated_at")
                .bind(&r.node_id).bind(head).bind(&r.now).bind(&r.now)
                .execute(&mut *tx).await?;
            let mut new_blocks = 0_u64;
            for &height in &r.blocks {
                let watermark: Option<i64> = sqlx::query_scalar("SELECT historical_high_watermark FROM block_history_state WHERE node_id=$1")
                    .bind(&r.node_id).fetch_optional(&mut *tx).await?;
                let known_hash: Option<String> = sqlx::query_scalar("SELECT block_hash FROM block_identity_window WHERE node_id=$1 AND height=$2")
                    .bind(&r.node_id).bind(height).fetch_optional(&mut *tx).await?;
                // Plain replay at or below the high-water mark never rewrites history.
                if watermark.is_some_and(|w| w > 0 && height <= w) {
                    continue;
                }
                let hash = block_hash(&r.node_id, height);
                let inserted = sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, node_key_fingerprint, node_key_valid_from, node_key_valid_until, node_key_history_complete, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, accepted_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26) ON CONFLICT DO NOTHING")
                    .bind(&r.node_id).bind(height).bind(&hash).bind(block_hash(&r.node_id, height - 1))
                    .bind(format!("0x{}", "ab".repeat(32))).bind(210_425_i64).bind(100_i64).bind("lat")
                    .bind(1_700_000_000_000_i64 + height * 1_000).bind(&r.now).bind(height % 7).bind(Some(1_000_i64))
                    .bind("subscription").bind("lat1coinbase").bind(None::<String>).bind("unknown")
                    .bind(None::<String>).bind(None::<String>).bind(None::<String>).bind(0_i64)
                    .bind(None::<String>).bind(None::<String>).bind("unknown").bind(None::<String>)
                    .bind("no attribution evidence").bind(&r.now)
                    .execute(&mut *tx).await?;
                if inserted.rows_affected() == 0 {
                    continue;
                }
                new_blocks += 1;
                sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, last_accepted_summary_at, updated_at) VALUES ($1, $2, 1, $3, $4, $5, $6) ON CONFLICT(node_id) DO UPDATE SET historical_high_watermark=CASE WHEN block_history_state.historical_high_watermark >= excluded.historical_high_watermark THEN block_history_state.historical_high_watermark ELSE excluded.historical_high_watermark END, cumulative_block_count=block_history_state.cumulative_block_count + 1, cumulative_transaction_count=block_history_state.cumulative_transaction_count + excluded.cumulative_transaction_count, cumulative_self_seal_count=block_history_state.cumulative_self_seal_count + excluded.cumulative_self_seal_count, last_accepted_summary_at=excluded.last_accepted_summary_at, updated_at=excluded.updated_at")
                    .bind(&r.node_id).bind(height).bind(height % 7).bind(0_i64).bind(&r.now).bind(&r.now)
                    .execute(&mut *tx).await?;
                if known_hash.is_none() {
                    sqlx::query("INSERT INTO block_identity_window (node_id, height, block_hash, retained_until, observed_at) VALUES ($1, $2, $3, $4, $5)")
                        .bind(&r.node_id).bind(height).bind(&hash).bind(&r.now).bind(&r.now)
                        .execute(&mut *tx).await?;
                }
            }
            let receipt = receipt_bytes(r, new_blocks);
            sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
                .bind(&r.report_id).bind(&r.agent_id).bind(agent.agent_epoch).bind(&r.boot_id).bind(r.sequence)
                .bind(&r.body_sha256).bind("accepted").bind(&receipt).bind(&r.now)
                .execute(&mut *tx).await?;
            sqlx::query("UPDATE agent_boots SET status='closed', closed_at=COALESCE(closed_at, $1), updated_at=$2 WHERE agent_id=$3 AND agent_epoch=$4 AND boot_id<>$5 AND status IN ('active','closing')")
                .bind(&r.now).bind(&r.now).bind(&r.agent_id).bind(agent.agent_epoch).bind(&r.boot_id)
                .execute(&mut *tx).await?;
            let lifecycle = "active";
            sqlx::query("INSERT INTO agent_boots (agent_id, agent_epoch, boot_id, status, previous_boot_id, last_sequence, close_report_id, closed_at, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $7='closed' THEN $8 ELSE NULL END, CASE WHEN $9='closed' THEN $10 ELSE NULL END, $11, $12) ON CONFLICT(agent_id, agent_epoch, boot_id) DO UPDATE SET status=excluded.status, previous_boot_id=COALESCE(excluded.previous_boot_id, agent_boots.previous_boot_id), last_sequence=CASE WHEN agent_boots.last_sequence >= excluded.last_sequence THEN agent_boots.last_sequence ELSE excluded.last_sequence END, close_report_id=COALESCE(excluded.close_report_id, agent_boots.close_report_id), closed_at=COALESCE(excluded.closed_at, agent_boots.closed_at), updated_at=excluded.updated_at")
                .bind(&r.agent_id).bind(agent.agent_epoch).bind(&r.boot_id).bind(lifecycle)
                .bind(None::<String>).bind(r.sequence).bind(lifecycle).bind(&r.report_id)
                .bind(lifecycle).bind(&r.now).bind(&r.now).bind(&r.now)
                .execute(&mut *tx).await?;
            sqlx::query("UPDATE agents SET active_boot_id=$1, active_boot_status=$2, previous_boot_id=$3, close_report_id=CASE WHEN $4='closed' THEN $5 ELSE close_report_id END, last_report_sequence=$6, last_inventory_revision=$7, inventory_sha256=$8, inventory_protocol_major=$9, last_received_at=$10, clock_skew_ms=$11, clock_status=$12, agent_capabilities_json=$13, shutdown_state=$14, shutdown_started_at=$15, shutdown_deadline_at=$16, shutdown_finished_at=$17, shutdown_unresolved_from=$18, shutdown_unresolved_to=$19, shutdown_last_error=$20, shutdown_forced=$21, shutdown_report_id=CASE WHEN $22='closed' THEN $23 ELSE shutdown_report_id END, shutdown_report_sequence=$24, shutdown_updated_at=$25, updated_at=$26 WHERE agent_id=$27")
                .bind(&r.boot_id).bind(lifecycle).bind(None::<String>).bind(lifecycle).bind(&r.report_id)
                .bind(r.sequence).bind(revision).bind(&r.inventory_sha256).bind(2_i64)
                .bind(&r.now).bind(Some(12_i64)).bind("known").bind("[\"block_summaries\"]")
                .bind("running").bind(None::<String>).bind(None::<String>).bind(None::<String>)
                .bind(None::<i64>).bind(None::<i64>).bind(None::<String>).bind(i64::from(false))
                .bind(lifecycle).bind(&r.report_id).bind(r.sequence).bind(&r.now).bind(&r.now)
                .bind(&r.agent_id)
                .execute(&mut *tx).await?;
            let audit_event_id: i64 = sqlx::query_scalar("INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING audit_event_id")
                .bind(None::<String>).bind("report_accepted").bind("agent").bind(&r.agent_id)
                .bind(None::<String>).bind(Some(format!("{{\"report_id\":\"{}\"}}", r.report_id))).bind(&r.now)
                .fetch_one(&mut *tx).await?;
            tx.commit().await?;
            Ok(SliceOutcome::Accepted { receipt, new_blocks, audit_event_id })
        }
    };
}

define_slice!(ingest_any, AnyConnection);
define_slice!(ingest_sqlite, SqliteConnection);

fn reports(agent_id: &str, count: usize, blocks_per_report: i64) -> Vec<SliceReport> {
    (0..count)
        .map(|i| {
            let seq = i as i64 + 1;
            // Each report overlaps the previous one by one block, exercising
            // the high-water dedup on every report.
            let start = (seq - 1) * blocks_per_report;
            SliceReport {
                agent_id: agent_id.to_owned(),
                report_id: format!("{agent_id}-report-{seq:06}"),
                boot_id: format!("{agent_id}-boot-1"),
                sequence: seq,
                body_sha256: format!("0x{seq:064x}"),
                node_id: format!("{agent_id}-node-1"),
                inventory_sha256: if seq < 3 {
                    "0xinv-a".into()
                } else {
                    "0xinv-b".into()
                },
                blocks: (start.max(1)..=start + blocks_per_report).collect(),
                now: format!("2026-10-22T02:{:02}:{:02}Z", (seq / 60) % 60, seq % 60),
            }
        })
        .collect()
}

/// Dump every column of the tables the slice writes, decoded generically,
/// so the two backends can be compared row for row.
async fn dump(conn: &mut AnyConnection) -> Vec<String> {
    let queries = [
        "SELECT agent_id, agent_epoch, active_boot_id, active_boot_status, last_report_sequence, last_inventory_revision, inventory_sha256, inventory_protocol_major, last_received_at, clock_skew_ms, clock_status, shutdown_state, shutdown_forced, updated_at FROM agents ORDER BY agent_id",
        "SELECT * FROM agent_boots ORDER BY agent_id, boot_id",
        "SELECT * FROM agent_report_receipts ORDER BY report_id",
        "SELECT * FROM nodes ORDER BY node_id",
        "SELECT * FROM component_status ORDER BY agent_id, scope_key, component_key",
        "SELECT * FROM block_summaries ORDER BY node_id, block_number",
        "SELECT * FROM block_history_state ORDER BY node_id",
        "SELECT * FROM block_identity_window ORDER BY node_id, height",
        "SELECT * FROM audit_events ORDER BY audit_event_id",
    ];
    let mut out = Vec::new();
    for q in queries {
        for row in sqlx::query(q).fetch_all(&mut *conn).await.unwrap() {
            let mut cells = Vec::new();
            for i in 0..row.len() {
                let cell = if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
                    format!("{v:?}")
                } else if let Ok(v) = row.try_get::<Option<String>, _>(i) {
                    format!("{v:?}")
                } else if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
                    format!(
                        "{:?}",
                        v.map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>())
                    )
                } else {
                    format!("{:?}", row.try_get::<Option<f64>, _>(i).unwrap())
                };
                cells.push(cell);
            }
            out.push(format!("{q}: {}", cells.join("|")));
        }
    }
    out
}

async fn seed_slice(conn: &mut AnyConnection, agent_id: &str) {
    seed_agent(conn, agent_id).await;
    seed_network(conn).await;
}

#[tokio::test]
async fn slice_parity_replay_and_dedup() {
    let mut dumps = Vec::new();
    for backend in backends().await {
        let kind = backend.kind;
        let mut conn = backend.pool.acquire().await.unwrap();
        seed_slice(&mut conn, "agent-a").await;
        let batch = reports("agent-a", 12, 3);
        let mut accepted_blocks = 0;
        for report in &batch {
            match ingest_any(&mut conn, report).await.unwrap() {
                SliceOutcome::Accepted { new_blocks, .. } => accepted_blocks += new_blocks,
                other => panic!("{kind:?}: {other:?}"),
            }
        }
        // 12 reports x 3 blocks, each overlapping by one → 3 + 11*3 = 36 heights 1..=36.
        assert_eq!(accepted_blocks, 36, "{kind:?}");
        // Exact replay returns the stored receipt bytes and changes nothing.
        let before = dump(&mut conn).await;
        let first = ingest_any(&mut conn, &batch[4]).await.unwrap();
        let SliceOutcome::Replayed { receipt } = first else {
            panic!("{kind:?}: {first:?}")
        };
        assert_eq!(receipt, receipt_bytes(&batch[4], 3));
        // Same report_id, different body → identity conflict.
        let mut forged = batch[4].clone();
        forged.body_sha256 = "0xforged".into();
        assert_eq!(
            ingest_any(&mut conn, &forged).await.unwrap(),
            SliceOutcome::Conflict
        );
        // New report id replaying old heights does not re-accumulate.
        let mut resync = batch[11].clone();
        resync.report_id = "agent-a-resync".into();
        resync.sequence = 99;
        resync.blocks = (1..=36).collect();
        let SliceOutcome::Accepted { new_blocks, .. } =
            ingest_any(&mut conn, &resync).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(new_blocks, 0, "{kind:?}");
        let count: i64 = sqlx::query_scalar("SELECT cumulative_block_count FROM block_history_state WHERE node_id = 'agent-a-node-1'")
            .fetch_one(&mut *conn).await.unwrap();
        assert_eq!(count, 36, "{kind:?}");
        // Belt behind the watermark: with the watermark cleared, the replayed
        // heights reach the INSERT, conflict, report rows_affected == 0, and
        // still accumulate nothing.
        conn.execute("UPDATE block_history_state SET historical_high_watermark = 0 WHERE node_id = 'agent-a-node-1'").await.unwrap();
        let mut belt = resync.clone();
        belt.report_id = "agent-a-belt".into();
        belt.sequence = 100;
        let SliceOutcome::Accepted { new_blocks, .. } = ingest_any(&mut conn, &belt).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(new_blocks, 0, "{kind:?}: conflicting inserts report 0 rows");
        let count: i64 = sqlx::query_scalar("SELECT cumulative_block_count FROM block_history_state WHERE node_id = 'agent-a-node-1'")
            .fetch_one(&mut *conn).await.unwrap();
        assert_eq!(count, 36, "{kind:?}");
        conn.execute("UPDATE block_history_state SET historical_high_watermark = 36 WHERE node_id = 'agent-a-node-1'").await.unwrap();
        let after = dump(&mut conn).await;
        assert_eq!(
            before.len() + 4,
            after.len(),
            "{kind:?}: receipt + audit row per accepted replay only"
        );
        dumps.push((kind, after));
    }
    if let [(_, sqlite), (_, pg)] = dumps.as_slice() {
        assert_eq!(sqlite.len(), pg.len());
        for (a, b) in sqlite.iter().zip(pg) {
            assert_eq!(a, b, "backend state diverged");
        }
        eprintln!(
            "[slice] SQLite and PostgreSQL states identical across {} rows",
            sqlite.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Q6 — performance
// ---------------------------------------------------------------------------

struct Stats {
    label: String,
    per_report: Vec<Duration>,
}

impl Stats {
    fn line(&mut self) -> String {
        self.per_report.sort();
        let n = self.per_report.len();
        let total: Duration = self.per_report.iter().sum();
        let pct = |p: f64| self.per_report[((n as f64 * p) as usize).min(n - 1)];
        format!(
            "{:<34} n={n} mean={:>7.1}µs p50={:>7.1}µs p95={:>7.1}µs",
            self.label,
            total.as_secs_f64() * 1e6 / n as f64,
            pct(0.50).as_secs_f64() * 1e6,
            pct(0.95).as_secs_f64() * 1e6,
        )
    }
}

async fn bench_native_sqlite(path: &Path, sync: SqliteSynchronous, batch: &[SliceReport]) -> Stats {
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true)
            .synchronous(sync)
            .busy_timeout(Duration::from_secs(5)),
    )
    .await
    .unwrap();
    let mut per_report = Vec::with_capacity(batch.len());
    for r in batch {
        let start = Instant::now();
        let outcome = ingest_sqlite(&mut conn, r).await.unwrap();
        per_report.push(start.elapsed());
        assert!(matches!(outcome, SliceOutcome::Accepted { .. }));
    }
    conn.close().await.unwrap();
    Stats {
        label: format!("native SqliteConnection ({sync:?})"),
        per_report,
    }
}

async fn bench_any(conn: &mut AnyConnection, label: String, batch: &[SliceReport]) -> Stats {
    let mut per_report = Vec::with_capacity(batch.len());
    for r in batch {
        let start = Instant::now();
        let outcome = ingest_any(conn, r).await.unwrap();
        per_report.push(start.elapsed());
        assert!(matches!(outcome, SliceOutcome::Accepted { .. }));
    }
    Stats { label, per_report }
}

#[tokio::test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
async fn q6_slice_benchmark() {
    const N: usize = 2_000;
    const BLOCKS: i64 = 4;
    let mut lines = Vec::new();
    for sync in [SqliteSynchronous::Full, SqliteSynchronous::Off] {
        for round in 0..2 {
            // Fresh databases per variant; alternate order to cancel warm-up bias.
            let dir = TempDir::new().unwrap();
            let native_path = migrated_sqlite_file(&dir.path().join("native").tap_mkdir()).await;
            let any_path = migrated_sqlite_file(&dir.path().join("any").tap_mkdir()).await;
            for path in [&native_path, &any_path] {
                let mut c = any_sqlite_pool(path, 5_000)
                    .await
                    .acquire()
                    .await
                    .unwrap()
                    .detach();
                seed_slice(&mut c, "agent-a").await;
                c.close().await.unwrap();
            }
            let batch = reports("agent-a", N, BLOCKS);
            let run_native = || bench_native_sqlite(&native_path, sync, &batch);
            let run_any = || async {
                let pool = AnyPoolOptions::new()
                    .max_connections(1)
                    .after_connect(move |conn, _| {
                        Box::pin(async move {
                            let mode = if sync == SqliteSynchronous::Off {
                                "OFF"
                            } else {
                                "FULL"
                            };
                            conn.execute(format!("PRAGMA synchronous={mode}").as_str())
                                .await?;
                            conn.execute("PRAGMA busy_timeout=5000").await?;
                            Ok(())
                        })
                    })
                    .connect(&sqlite_url(&any_path))
                    .await
                    .unwrap();
                let mut conn = pool.acquire().await.unwrap();
                bench_any(
                    &mut conn,
                    format!("AnyConnection over SQLite ({sync:?})"),
                    &batch,
                )
                .await
            };
            let (mut a, mut b) = if round == 0 {
                let a = run_native().await;
                (a, run_any().await)
            } else {
                let b = run_any().await;
                (run_native().await, b)
            };
            lines.push(format!("round {round}: {}", a.line()));
            lines.push(format!("round {round}: {}", b.line()));
        }
    }
    if let Some(pg) = pg_backend().await {
        let mut conn = pg.pool.acquire().await.unwrap();
        seed_slice(&mut conn, "agent-a").await;
        let batch = reports("agent-a", N, BLOCKS);
        let version: String = sqlx::query_scalar("SELECT current_setting('server_version')")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let label = format!("AnyConnection over PostgreSQL {version}");
        let mut s = bench_any(&mut conn, label, &batch).await;
        lines.push(s.line());
    }
    for line in lines {
        eprintln!("[bench] {line}");
    }
}

trait TapMkdir {
    fn tap_mkdir(self) -> PathBuf;
}

impl TapMkdir for PathBuf {
    fn tap_mkdir(self) -> PathBuf {
        std::fs::create_dir_all(&self).unwrap();
        self
    }
}
