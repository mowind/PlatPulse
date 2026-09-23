#![cfg(unix)]
//! Subprocess coverage for the coordinated upgrade checkpoint (issue #190).
//!
//! These tests run the real platpulse-server binary against an independent
//! temporary Server deployment and an exact Agent half, mirroring the CLI
//! ownership regression style of cli_ownership.rs: a held ownership guard must
//! refuse the command before SQLite opens and leave the database untouched.
//! They also cover missing/corrupt artifacts, isolated restore consistency, and
//! the fact that the checkpoint preserves a sensitive receipt value verbatim
//! where the sanitized Admin Backup rewrites it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use platpulse_core::inventory::{InventoryNode, NodeInventory};
use platpulse_server::checkpoint::{AgentCheckpointManifest, CheckpointArtifact};
use platpulse_server::database::{ServerDatabaseConfig, initialize};
use platpulse_server::secrets::create_pepper_file;
use sha2::{Digest, Sha256};
use sqlx::Connection;
use tempfile::TempDir;

const AGENT_ID: &str = "0195f2a1-0011-4011-8011-000000000011";
const NODE_ID: &str = "0195f2a1-0014-4014-8014-000000000014";
const CLOSED_BOOT: &str = "0195f2a1-0012-4012-8012-000000000012";
const NEXT_BOOT: &str = "0195f2a1-0013-4013-8013-000000000013";
const CLOSING_ID: &str = "0195f2a1-0061-4061-8061-000000000061";
const SENSITIVE_RECEIPT: &[u8] =
    br#"{"report_id":"0195f2a1-0061-4061-8061-000000000061","credential":"pp_agent_topsecretvalue"}"#;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sha256_file(path: &Path) -> String {
    sha256_hex(&fs::read(path).unwrap())
}

fn closing_body() -> Vec<u8> {
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    report["agent_id"] = serde_json::Value::String(AGENT_ID.to_owned());
    report["report_id"] = serde_json::Value::String(CLOSING_ID.to_owned());
    report["boot_id"] = serde_json::Value::String(CLOSED_BOOT.to_owned());
    report["report_sequence"] = serde_json::json!(4);
    report["boot_transition"] = serde_json::json!("closing");
    serde_json::to_vec(&report).unwrap()
}

fn declared_inventory() -> NodeInventory {
    NodeInventory {
        revision: 1,
        nodes: vec![InventoryNode {
            node_id: NODE_ID.parse().unwrap(),
            display_name: None,
            network_key: "platon-mainnet".parse().unwrap(),
            rpc_endpoint: "ws://127.0.0.1:6790".parse().unwrap(),
            process: None,
        }],
    }
}

fn artifact(role: &str, file: &str, original: &Path, base: &Path) -> CheckpointArtifact {
    let path = base.join(file);
    CheckpointArtifact {
        role: role.to_owned(),
        original_path: original.display().to_string(),
        file: file.to_owned(),
        bytes: fs::metadata(&path).unwrap().len() as i64,
        sha256: sha256_file(&path),
    }
}

/// Write an exact Agent half whose facts match the seeded Server state.
async fn write_agent_checkpoint(base: &Path, declaration_json: &str, inventory_sha256: &str) {
    fs::create_dir_all(base).unwrap();
    let body = closing_body();

    let store_path = base.join("agent-store.sqlite");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&store_path)
        .create_if_missing(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE agent_state (singleton INTEGER PRIMARY KEY, agent_id TEXT, agent_epoch INTEGER, boot_id TEXT, boot_state TEXT, previous_boot_id TEXT, pending_transition TEXT, pending_previous_boot_id TEXT, close_report_id TEXT, last_report_body BLOB)",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE upgrade_preparation (singleton INTEGER PRIMARY KEY, agent_id TEXT, agent_epoch INTEGER, closing_report_id TEXT, closing_report_sequence INTEGER, closing_receipt_disposition TEXT, inventory_revision INTEGER, inventory_sha256 TEXT, declaration_json TEXT, closed_boot_id TEXT, next_boot_id TEXT, pending_transition TEXT, accepted_inventory_revision INTEGER, accepted_inventory_sha256 TEXT, accepted_inventory_protocol_major INTEGER)",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    // The prepared Store has an empty immutable-delivery spool.
    sqlx::query("CREATE TABLE reports (report_id TEXT PRIMARY KEY)")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id, last_report_body) VALUES (1, ?, 1, ?, 'drained_pending', ?, 'drained_previous', ?, ?, ?)",
    )
    .bind(AGENT_ID)
    .bind(NEXT_BOOT)
    .bind(CLOSED_BOOT)
    .bind(CLOSED_BOOT)
    .bind(CLOSING_ID)
    .bind(&body)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO upgrade_preparation (singleton, agent_id, agent_epoch, closing_report_id, closing_report_sequence, closing_receipt_disposition, inventory_revision, inventory_sha256, declaration_json, closed_boot_id, next_boot_id, pending_transition, accepted_inventory_revision, accepted_inventory_sha256, accepted_inventory_protocol_major) VALUES (1, ?, 1, ?, 4, 'accepted', 1, ?, ?, ?, ?, 'drained_previous', 1, ?, 1)",
    )
    .bind(AGENT_ID)
    .bind(CLOSING_ID)
    .bind(inventory_sha256)
    .bind(declaration_json)
    .bind(CLOSED_BOOT)
    .bind(NEXT_BOOT)
    .bind(inventory_sha256)
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    fs::write(
        base.join("agent.toml"),
        b"server_url=\"https://example.com\"\n",
    )
    .unwrap();
    fs::write(base.join("credential"), b"pp_agent_topsecretvalue\n").unwrap();

    let manifest = AgentCheckpointManifest {
        kind: "platpulse.upgrade-checkpoint.agent".to_owned(),
        format_version: 1,
        agent_version: "0.1.0".to_owned(),
        created_at: "2026-08-12T09:00:00Z".to_owned(),
        agent_id: AGENT_ID.to_owned(),
        agent_epoch: 1,
        inventory_protocol_major: 1,
        closed_boot_id: CLOSED_BOOT.to_owned(),
        next_boot_id: NEXT_BOOT.to_owned(),
        pending_transition: "drained_previous".to_owned(),
        closing_report_id: CLOSING_ID.to_owned(),
        closing_report_sequence: 4,
        closing_receipt_disposition: "accepted".to_owned(),
        inventory_revision: 1,
        inventory_sha256: inventory_sha256.to_owned(),
        accepted_inventory_revision: 1,
        accepted_inventory_sha256: inventory_sha256.to_owned(),
        accepted_inventory_protocol_major: 1,
        declaration_sha256: sha256_hex(declaration_json.as_bytes()),
        declaration_json: declaration_json.to_owned(),
        closing_report_body_sha256: format!("0x{}", sha256_hex(&body)),
        artifacts: vec![
            artifact("agent-config", "agent.toml", &base.join("agent.toml"), base),
            artifact(
                "agent-credential",
                "credential",
                &base.join("credential"),
                base,
            ),
            artifact(
                "agent-store",
                "agent-store.sqlite",
                &base.join("agent-store.sqlite"),
                base,
            ),
        ],
    };
    fs::write(
        base.join("agent-checkpoint.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

struct Deployment {
    _dir: TempDir,
    state: PathBuf,
    db: PathBuf,
    config: PathBuf,
    backups: PathBuf,
    agent_checkpoint: PathBuf,
}

impl Deployment {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let backups = dir.path().join("backups");
        for path in [&state, &backups] {
            fs::create_dir_all(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db = state.join("server.db");
        let config = state.join("server.toml");
        fs::write(
            &config,
            format!(
                "state_dir = \"{state}\"\ndb_path = \"{db}\"\npepper_file = \"{pepper}\"\nbackup_dir = \"{backups}\"\n",
                state = state.display(),
                db = db.display(),
                pepper = state.join("server-pepper").display(),
                backups = backups.display(),
            ),
        )
        .unwrap();
        create_pepper_file(&state.join("server-pepper")).unwrap();

        let inventory = declared_inventory();
        let declaration_json = serde_json::to_string(&inventory).unwrap();
        let inventory_sha256 = inventory.content_sha256().to_string();
        let body = closing_body();

        let database = initialize(ServerDatabaseConfig::for_deployment(&db, false))
            .await
            .unwrap();
        let now = "2026-08-12T09:00:00Z";
        sqlx::query(
            "INSERT INTO agents (agent_id, agent_epoch, active_boot_id, active_boot_status, close_report_id, last_report_sequence, last_inventory_revision, inventory_sha256, inventory_protocol_major, created_at, updated_at) VALUES (?, 1, ?, 'closed', ?, 4, 1, ?, 1, ?, ?)",
        )
        .bind(AGENT_ID)
        .bind(CLOSED_BOOT)
        .bind(CLOSING_ID)
        .bind(&inventory_sha256)
        .bind(now)
        .bind(now)
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_boots (agent_id, agent_epoch, boot_id, status, last_sequence, close_report_id, closed_at, created_at, updated_at) VALUES (?, 1, ?, 'closed', 4, ?, ?, ?, ?)",
        )
        .bind(AGENT_ID)
        .bind(CLOSED_BOOT)
        .bind(CLOSING_ID)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES (?, ?, 1, ?, 4, ?, 'accepted', ?, ?)",
        )
        .bind(CLOSING_ID)
        .bind(AGENT_ID)
        .bind(CLOSED_BOOT)
        .bind(format!("0x{}", sha256_hex(&body)))
        .bind(SENSITIVE_RECEIPT)
        .bind(now)
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO deleted_nodes (node_id, agent_id, network_key, display_name, deleted_by_user_id, deleted_at) VALUES (?, ?, 'platon-mainnet', 'purged', NULL, ?)",
        )
        .bind(NODE_ID)
        .bind(AGENT_ID)
        .bind(now)
        .execute(database.pool())
        .await
        .unwrap();
        database.close().await;

        let agent_checkpoint = dir.path().join("agent-checkpoint");
        write_agent_checkpoint(&agent_checkpoint, &declaration_json, &inventory_sha256).await;

        Self {
            _dir: dir,
            state,
            db,
            config,
            backups,
            agent_checkpoint,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_platpulse-server"));
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.output().unwrap()
    }

    fn path(&self, value: &Path) -> String {
        value.display().to_string()
    }

    fn create_checkpoint(&self, output: &Path) -> Output {
        self.run(&[
            "checkpoint",
            "create",
            "--config",
            &self.path(&self.config),
            "--agent-checkpoint",
            &self.path(&self.agent_checkpoint),
            "--output",
            &self.path(output),
        ])
    }

    fn verify_checkpoint(&self, checkpoint: &Path) -> Output {
        self.run(&[
            "checkpoint",
            "verify",
            "--checkpoint",
            &self.path(checkpoint),
        ])
    }

    fn restore_checkpoint(&self, checkpoint: &Path, restore: &Path) -> Output {
        self.run(&[
            "checkpoint",
            "restore",
            "--checkpoint",
            &self.path(checkpoint),
            "--restore-dir",
            &self.path(restore),
        ])
    }
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fingerprint(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| sha256_hex(&bytes))
}

fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn database_fingerprint(db: &Path) -> [Option<String>; 3] {
    [
        fingerprint(db),
        fingerprint(&sidecar(db, "-wal")),
        fingerprint(&sidecar(db, "-shm")),
    ]
}

async fn receipt_body(path: &Path) -> Vec<u8> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let body: Vec<u8> =
        sqlx::query_scalar("SELECT receipt_body FROM agent_report_receipts WHERE report_id = ?")
            .bind(CLOSING_ID)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    connection.close().await.unwrap();
    body
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[tokio::test]
async fn checkpoint_round_trip_preserves_the_exact_receipt_and_restores_isolation() {
    let deployment = Deployment::new().await;
    let output = deployment.state.join("checkpoint");

    let created = deployment.create_checkpoint(&output);
    assert_success(&created, "checkpoint create");
    let verified = deployment.verify_checkpoint(&output);
    assert_success(&verified, "checkpoint verify");

    // The checkpoint preserves the exact Report Receipt, including a value the
    // sanitized Admin Backup rewrites.
    let checkpoint_db = output.join("server").join("server.db");
    assert_eq!(receipt_body(&checkpoint_db).await, SENSITIVE_RECEIPT);
    let credential = fs::read(output.join("agent").join("credential")).unwrap();
    assert_eq!(credential, b"pp_agent_topsecretvalue\n");

    // Sensitive artifacts are owner-only and directories are private.
    for file in [
        output.join("checkpoint.json"),
        output.join("agent").join("agent-store.sqlite"),
        output.join("agent").join("credential"),
        output.join("server").join("server-pepper"),
    ] {
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "unexpected mode for {}", file.display());
    }
    for directory in [output.clone(), output.join("agent"), output.join("server")] {
        let mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "unexpected mode for {}", directory.display());
    }

    // Isolated restore reproduces the same closed-Boot checkpoint.
    let restore = deployment.state.join("restore");
    let restored = deployment.restore_checkpoint(&output, &restore);
    assert_success(&restored, "checkpoint restore");
    assert_eq!(
        receipt_body(&restore.join("server").join("server.db")).await,
        SENSITIVE_RECEIPT
    );
    // The isolated restore adds path-rewritten config copies so it can be
    // pointed at without reopening the live database.
    let isolated = fs::read_to_string(restore.join("server").join("server.isolated.toml")).unwrap();
    assert!(
        isolated.contains(
            &restore
                .join("server")
                .join("server.db")
                .display()
                .to_string()
        )
    );
    assert!(restore.join("agent").join("agent.isolated.toml").is_file());

    // The existing sanitized Admin Backup redacts the same value, which is why
    // it cannot serve as the exact coordinated checkpoint.
    let backed_up = deployment.run(&["backup", "--config", &deployment.path(&deployment.config)]);
    assert_success(&backed_up, "sanitized backup");
    let artifacts: Vec<PathBuf> = fs::read_dir(&deployment.backups)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(artifacts.len(), 1);
    let sanitized = receipt_body(&artifacts[0]).await;
    assert_ne!(sanitized, SENSITIVE_RECEIPT);
    assert!(
        !String::from_utf8_lossy(&sanitized).contains("pp_agent_topsecretvalue"),
        "the sanitized backup leaked the sensitive value"
    );
}

#[tokio::test]
async fn a_running_server_owner_refuses_checkpoint_create_before_sqlite() {
    let deployment = Deployment::new().await;
    let output = deployment.state.join("refused-checkpoint");

    let guard = platpulse_server::ownership::acquire(&deployment.db).unwrap();
    let before = database_fingerprint(&deployment.db);
    let refused = deployment.create_checkpoint(&output);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("running PlatPulse Server owns") || stderr.contains("exclusive stopped"),
        "unexpected refusal: {stderr}"
    );
    assert_eq!(
        database_fingerprint(&deployment.db),
        before,
        "a refused checkpoint modified the Server database"
    );
    assert!(!output.exists(), "a refused checkpoint created output");
    drop(guard);

    // The same command works once the guard is released.
    let created = deployment.create_checkpoint(&output);
    assert_success(&created, "checkpoint create after release");
    assert!(platpulse_server::ownership::acquire(&deployment.db).is_ok());
}

#[tokio::test]
async fn missing_or_corrupt_artifacts_fail_verification_and_restore() {
    let deployment = Deployment::new().await;
    let output = deployment.state.join("checkpoint");
    assert_success(&deployment.create_checkpoint(&output), "checkpoint create");

    let missing = deployment.state.join("missing");
    copy_tree(&output, &missing);
    fs::remove_file(missing.join("agent").join("credential")).unwrap();
    assert!(!deployment.verify_checkpoint(&missing).status.success());

    let corrupt_store = deployment.state.join("corrupt-store");
    copy_tree(&output, &corrupt_store);
    {
        let path = corrupt_store.join("agent").join("agent-store.sqlite");
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
    }
    assert!(
        !deployment
            .verify_checkpoint(&corrupt_store)
            .status
            .success(),
        "a corrupt Agent Store passed verification"
    );

    let corrupt_db = deployment.state.join("corrupt-db");
    copy_tree(&output, &corrupt_db);
    {
        let path = corrupt_db.join("server").join("server.db");
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
    }
    assert!(
        !deployment.verify_checkpoint(&corrupt_db).status.success(),
        "a corrupt Server database passed verification"
    );
    let refused = deployment.state.join("corrupt-db-restore");
    assert!(
        !deployment
            .restore_checkpoint(&corrupt_db, &refused)
            .status
            .success()
    );
    assert!(
        !refused.exists(),
        "a refused restore left a partial destination"
    );

    let missing_manifest = deployment.state.join("missing-manifest");
    copy_tree(&output, &missing_manifest);
    fs::remove_file(missing_manifest.join("checkpoint.json")).unwrap();
    assert!(
        !deployment
            .verify_checkpoint(&missing_manifest)
            .status
            .success()
    );
}

#[tokio::test]
async fn vacuum_into_captures_committed_wal_content() {
    // The coordinated Server snapshot uses VACUUM INTO. Prove that a committed
    // write still sitting in an uncheckpointed WAL is included, rather than
    // ignored, when only the main database file is copied.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("source.db");
    let target = dir.path().join("target.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&source)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .pragma("wal_autocheckpoint", "0");
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE t (value TEXT)")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO t VALUES ('committed-in-wal')")
        .execute(&mut connection)
        .await
        .unwrap();

    let mut wal_name = source.as_os_str().to_owned();
    wal_name.push("-wal");
    let wal = PathBuf::from(wal_name);
    assert!(
        fs::metadata(&wal)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false),
        "the write did not stay in the WAL"
    );

    sqlx::query(&format!("VACUUM INTO '{}'", target.display()))
        .execute(&mut connection)
        .await
        .unwrap();
    let mut snapshot = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&target)
            .read_only(true),
    )
    .await
    .unwrap();
    let value: String = sqlx::query_scalar("SELECT value FROM t")
        .fetch_one(&mut snapshot)
        .await
        .unwrap();
    assert_eq!(value, "committed-in-wal");
}
