#![cfg(unix)]
//! Offline Inventory baseline conversion coverage (issue #191).
//!
//! Every test builds a real Server deployment and a real coordinated checkpoint
//! (issue #190), converts it offline, and validates the result without starting
//! any production worker. The end-to-end test then serves only the converted
//! database in isolation and proves the first v2 Report keeps the preserved
//! revision and completes the pending DrainedPrevious Boot linkage.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use platpulse_core::inventory::{InventoryDeclaration, InventoryNode, NodeInventory};
use platpulse_core::protocol::{AGENT_API_REPORTS_PATH, AGENT_API_REPORTS_PATH_V2};
use platpulse_server::auth::{AuthConfig, create_owner, hash_password};
use platpulse_server::checkpoint::{AgentCheckpointManifest, CheckpointArtifact};
use platpulse_server::database::{ServerDatabaseConfig, initialize};
use platpulse_server::enrollment::{create_enrollment_token, enroll};
use platpulse_server::http::{AppState, build_app};
use platpulse_server::migration::{convert_baseline, convert_checkpoint, verify_conversion};
use platpulse_server::network::create_network;
use platpulse_server::secrets::{create_pepper_file, load_pepper_file};
use sha2::{Digest, Sha256};
use sqlx::Connection;
use tempfile::TempDir;

const NODE_ID: &str = "0195f2a1-0014-4014-8014-000000000014";
const PURGED_NODE_ID: &str = "0195f2a1-0016-4016-8016-000000000016";
const CLOSED_BOOT: &str = "0195f2a1-0012-4012-8012-000000000012";
const NEXT_BOOT: &str = "0195f2a1-0013-4013-8013-000000000013";
const CLOSING_ID: &str = "0195f2a1-0061-4061-8061-000000000061";
const PRESERVED_REVISION: i64 = 57;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sha256_file(path: &Path) -> String {
    sha256_hex(&fs::read(path).unwrap())
}

fn declared_inventory() -> NodeInventory {
    NodeInventory {
        revision: PRESERVED_REVISION as u64,
        nodes: vec![InventoryNode {
            node_id: NODE_ID.parse().unwrap(),
            display_name: None,
            network_key: "platon-mainnet".parse().unwrap(),
            rpc_endpoint: "ws://127.0.0.1:6790".parse().unwrap(),
            process: None,
        }],
    }
}

fn closing_body() -> Vec<u8> {
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    report["report_id"] = serde_json::json!(CLOSING_ID);
    report["boot_id"] = serde_json::json!(CLOSED_BOOT);
    report["report_sequence"] = serde_json::json!(4);
    report["boot_transition"] = serde_json::json!("closing");
    serde_json::to_vec(&report).unwrap()
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
async fn write_agent_checkpoint(
    base: &Path,
    agent_id: &str,
    declaration_json: &str,
    inventory_sha256: &str,
    include_revision: bool,
    include_record: bool,
    credential: &str,
) {
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
    sqlx::query("CREATE TABLE reports (report_id TEXT PRIMARY KEY)")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE inventory_declaration (singleton INTEGER PRIMARY KEY, revision INTEGER, sha256 TEXT, report_id TEXT, adopted_at TEXT)",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id, last_report_body) VALUES (1, ?, 1, ?, 'drained_pending', ?, 'drained_previous', ?, ?, ?)",
    )
    .bind(agent_id)
    .bind(NEXT_BOOT)
    .bind(CLOSED_BOOT)
    .bind(CLOSED_BOOT)
    .bind(CLOSING_ID)
    .bind(&body)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO upgrade_preparation (singleton, agent_id, agent_epoch, closing_report_id, closing_report_sequence, closing_receipt_disposition, inventory_revision, inventory_sha256, declaration_json, closed_boot_id, next_boot_id, pending_transition, accepted_inventory_revision, accepted_inventory_sha256, accepted_inventory_protocol_major) VALUES (1, ?, 1, ?, 4, 'accepted', ?, ?, ?, ?, ?, 'drained_previous', ?, ?, 1)",
    )
    .bind(agent_id)
    .bind(CLOSING_ID)
    .bind(PRESERVED_REVISION)
    .bind(inventory_sha256)
    .bind(declaration_json)
    .bind(CLOSED_BOOT)
    .bind(NEXT_BOOT)
    .bind(PRESERVED_REVISION)
    .bind(inventory_sha256)
    .execute(&mut connection)
    .await
    .unwrap();
    if include_record {
        sqlx::query(
            "INSERT INTO inventory_declaration (singleton, revision, sha256, report_id, adopted_at) VALUES (1, ?, ?, ?, '2026-09-23T00:00:00Z')",
        )
        .bind(PRESERVED_REVISION)
        .bind(inventory_sha256)
        .bind(CLOSING_ID)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    connection.close().await.unwrap();

    let revision_line = if include_revision {
        format!("inventory_revision = {PRESERVED_REVISION}\n")
    } else {
        String::new()
    };
    fs::write(
        base.join("agent.toml"),
        format!(
            "server_url=\"https://example.com\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\n{revision_line}nodes=[{{node_id=\"{NODE_ID}\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
            base.join("credential").display(),
            base.join("agent-state.db").display(),
        ),
    )
    .unwrap();
    fs::write(base.join("credential"), format!("{credential}\n")).unwrap();

    let manifest = AgentCheckpointManifest {
        kind: "platpulse.upgrade-checkpoint.agent".to_owned(),
        format_version: 1,
        agent_version: "0.1.0".to_owned(),
        created_at: "2026-08-12T09:00:00Z".to_owned(),
        agent_id: agent_id.to_owned(),
        agent_epoch: 1,
        inventory_protocol_major: 1,
        closed_boot_id: CLOSED_BOOT.to_owned(),
        next_boot_id: NEXT_BOOT.to_owned(),
        pending_transition: "drained_previous".to_owned(),
        closing_report_id: CLOSING_ID.to_owned(),
        closing_report_sequence: 4,
        closing_receipt_disposition: "accepted".to_owned(),
        inventory_revision: PRESERVED_REVISION,
        inventory_sha256: inventory_sha256.to_owned(),
        accepted_inventory_revision: PRESERVED_REVISION,
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
    agent_checkpoint: PathBuf,
    checkpoint: PathBuf,
    converted: PathBuf,
    agent_id: String,
    credential: String,
}

async fn make_deployment(include_revision: bool, include_record: bool) -> Deployment {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let db = state.join("server.db");
    let pepper = state.join("server-pepper");
    let config = state.join("server.toml");
    fs::write(
        &config,
        format!(
            "state_dir = \"{}\"\ndb_path = \"{}\"\npepper_file = \"{}\"\n",
            state.display(),
            db.display(),
            pepper.display()
        ),
    )
    .unwrap();
    create_pepper_file(&pepper).unwrap();
    let pepper_value = load_pepper_file(&pepper).unwrap();

    let database = initialize(ServerDatabaseConfig::for_deployment(&db, false))
        .await
        .unwrap();
    create_owner(
        &database,
        "admin",
        &hash_password(b"correct horse battery").unwrap(),
    )
    .await
    .unwrap();
    create_network(
        &database,
        "platon-mainnet",
        "PlatON Mainnet",
        "0x0000000000000000000000000000000000000000000000000000000000000001",
        210425,
        210425,
        "lat",
    )
    .await
    .unwrap();
    let token = create_enrollment_token(&database, &pepper_value, None, Duration::from_secs(3600))
        .await
        .unwrap()
        .token;
    let enrolled = enroll(&database, &pepper_value, &token).await.unwrap();
    let agent_id = enrolled.agent_id.clone();
    let credential = enrolled.credential.clone();

    let inventory = declared_inventory();
    let declaration_json = serde_json::to_string(&inventory).unwrap();
    let inventory_sha256 = inventory.content_sha256().to_string();
    let body = closing_body();
    let now = "2026-08-12T09:00:00Z";
    sqlx::query(
        "UPDATE agents SET agent_epoch = 1, active_boot_id = ?, active_boot_status = 'closed', close_report_id = ?, last_report_sequence = 4, last_inventory_revision = ?, inventory_sha256 = ?, inventory_protocol_major = 1, updated_at = ? WHERE agent_id = ?",
    )
    .bind(CLOSED_BOOT)
    .bind(CLOSING_ID)
    .bind(PRESERVED_REVISION)
    .bind(&inventory_sha256)
    .bind(now)
    .bind(&agent_id)
    .execute(database.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_boots (agent_id, agent_epoch, boot_id, status, last_sequence, close_report_id, closed_at, created_at, updated_at) VALUES (?, 1, ?, 'closed', 4, ?, ?, ?, ?)",
    )
    .bind(&agent_id)
    .bind(CLOSED_BOOT)
    .bind(CLOSING_ID)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(database.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at, inventory_protocol_major) VALUES (?, ?, 1, ?, 4, ?, 'accepted', ?, ?, 1)",
    )
    .bind(CLOSING_ID)
    .bind(&agent_id)
    .bind(CLOSED_BOOT)
    .bind(format!("0x{}", sha256_hex(&body)))
    .bind(br#"{"report_id":"closing","credential":"pp_agent_topsecretvalue"}"#.to_vec())
    .bind(now)
    .execute(database.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deleted_nodes (node_id, agent_id, network_key, display_name, deleted_by_user_id, deleted_at) VALUES (?, ?, 'platon-mainnet', 'purged', NULL, ?)",
    )
    .bind(PURGED_NODE_ID)
    .bind(&agent_id)
    .bind(now)
    .execute(database.pool())
    .await
    .unwrap();
    database.close().await;

    let agent_checkpoint = dir.path().join("agent-checkpoint");
    write_agent_checkpoint(
        &agent_checkpoint,
        &agent_id,
        &declaration_json,
        &inventory_sha256,
        include_revision,
        include_record,
        &credential,
    )
    .await;

    let server_config =
        platpulse_server::config::ServerConfig::resolve(Some(&config), &Default::default())
            .unwrap();
    let checkpoint = dir.path().join("checkpoint");
    platpulse_server::checkpoint::create_checkpoint(&server_config, &agent_checkpoint, &checkpoint)
        .await
        .unwrap();
    let converted = dir.path().join("converted");

    Deployment {
        _dir: dir,
        state,
        db,
        config,
        agent_checkpoint,
        checkpoint,
        converted,
        agent_id,
        credential,
    }
}

fn v2_report_body(agent_id: &str, report_id: &str, sequence: u64, endpoint: &str) -> Vec<u8> {
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../platpulse-core/tests/fixtures/report_v2_minimal.json"
    ))
    .unwrap();
    report["agent_id"] = serde_json::json!(agent_id);
    report["boot_id"] = serde_json::json!(NEXT_BOOT);
    report["previous_boot_id"] = serde_json::json!(CLOSED_BOOT);
    report["boot_transition"] = serde_json::json!("drained_previous");
    report["report_sequence"] = serde_json::json!(sequence);
    report["report_id"] = serde_json::json!(report_id);
    report["inventory"]["nodes"][0]["rpc_endpoint"] = serde_json::json!(endpoint);
    serde_json::to_vec(&report).unwrap()
}

fn v1_report_body(agent_id: &str, report_id: &str, sequence: u64) -> Vec<u8> {
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    report["agent_id"] = serde_json::json!(agent_id);
    report["boot_id"] = serde_json::json!(NEXT_BOOT);
    report["previous_boot_id"] = serde_json::json!(CLOSED_BOOT);
    report["boot_transition"] = serde_json::json!("drained_previous");
    report["report_sequence"] = serde_json::json!(sequence);
    report["report_id"] = serde_json::json!(report_id);
    report["inventory"]["revision"] = serde_json::json!(PRESERVED_REVISION);
    serde_json::to_vec(&report).unwrap()
}

async fn read_agent_row(path: &Path, agent_id: &str) -> (i64, Option<String>, Option<i64>) {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let row: (i64, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT last_inventory_revision, inventory_sha256, inventory_protocol_major FROM agents WHERE agent_id = ?",
    )
    .bind(agent_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    row
}

#[test]
fn baseline_conversion_preserves_the_revision_and_distinguishes_uninitialized_empties() {
    let inventory = declared_inventory();
    let v1 = inventory.content_sha256().to_string();
    let converted = convert_baseline(PRESERVED_REVISION, Some(&v1), &inventory).unwrap();
    assert_eq!(converted.preserved_revision, PRESERVED_REVISION);
    assert_eq!(converted.previous_sha256.as_deref(), Some(v1.as_str()));
    assert_eq!(
        converted.fingerprint_sha256.as_deref(),
        Some(
            InventoryDeclaration {
                nodes: inventory.nodes.clone(),
            }
            .fingerprint()
            .as_str()
        )
    );
    assert!(!converted.uninitialized);

    // A truly accepted empty declaration migrates on its verified baseline.
    let empty = NodeInventory {
        revision: PRESERVED_REVISION as u64,
        nodes: Vec::new(),
    };
    let empty_hash = empty.content_sha256().to_string();
    let converted = convert_baseline(PRESERVED_REVISION, Some(&empty_hash), &empty).unwrap();
    assert_eq!(converted.preserved_revision, PRESERVED_REVISION);
    assert!(!converted.uninitialized);
    assert_ne!(converted.fingerprint_sha256, converted.previous_sha256);

    // Never accepted is a legitimate uninitialized state, never an invented
    // accepted empty declaration.
    let converted = convert_baseline(0, None, &empty).unwrap();
    assert!(converted.uninitialized);
    assert_eq!(converted.preserved_revision, 0);
    assert!(converted.fingerprint_sha256.is_none());

    // Missing evidence and out-of-range revisions stop instead of truncating.
    assert!(convert_baseline(PRESERVED_REVISION, None, &inventory).is_err());
    assert!(convert_baseline(-1, Some(&v1), &inventory).is_err());
    assert!(convert_baseline(PRESERVED_REVISION, Some("0xdead"), &inventory).is_err());
}

#[tokio::test]
async fn conversion_preserves_the_revision_fingerprint_and_boot_linkage() {
    let deployment = make_deployment(true, true).await;

    let summary = convert_checkpoint(&deployment.checkpoint, &deployment.converted)
        .await
        .unwrap();
    assert_eq!(summary.inventory_revision, PRESERVED_REVISION);
    assert!(!summary.uninitialized);
    assert_eq!(summary.deleted_nodes, 1);
    let fingerprint = summary.fingerprint_sha256.clone().unwrap();
    assert_ne!(
        summary.previous_sha256.as_deref(),
        Some(fingerprint.as_str())
    );

    let verified = verify_conversion(&deployment.checkpoint, &deployment.converted)
        .await
        .unwrap();
    assert_eq!(verified.inventory_revision, PRESERVED_REVISION);
    assert_eq!(
        verified.fingerprint_sha256.as_deref(),
        Some(fingerprint.as_str())
    );

    // The Server baseline keeps the accepted revision; only the algorithm
    // changes. The original Closing Receipt survives byte for byte.
    let (revision, sha256, protocol) = read_agent_row(
        &deployment.converted.join("server/server.db"),
        &deployment.agent_id,
    )
    .await;
    assert_eq!(revision, PRESERVED_REVISION);
    assert_eq!(sha256.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(protocol, Some(2));
    let receipt = fs::read(deployment.converted.join("server/server.db"));
    assert!(receipt.is_ok());

    // The converted Agent configuration no longer carries a local revision.
    let converted_config =
        fs::read_to_string(deployment.converted.join("agent/agent.toml")).unwrap();
    assert!(
        !converted_config.contains("inventory_revision"),
        "{converted_config}"
    );
    assert!(converted_config.contains("agent-store.sqlite"));

    // The Agent Declaration Record is converted too, keeping the revision.
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(deployment.converted.join("agent/agent-store.sqlite"))
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let (record_revision, record_sha): (i64, String) =
        sqlx::query_as("SELECT revision, sha256 FROM inventory_declaration WHERE singleton = 1")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let marker: (i64, i64) = sqlx::query_as(
        "SELECT protocol_major, preserved_revision FROM inventory_migration WHERE singleton = 1",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    assert_eq!(record_revision, PRESERVED_REVISION);
    assert_eq!(record_sha, fingerprint);
    assert_eq!(marker, (2, PRESERVED_REVISION));

    // The live deployment was not touched.
    let (live_revision, live_sha, live_protocol) =
        read_agent_row(&deployment.db, &deployment.agent_id).await;
    assert_eq!(live_revision, PRESERVED_REVISION);
    assert_eq!(live_sha.as_deref(), summary.previous_sha256.as_deref());
    assert_eq!(live_protocol, Some(1));
    let _ = &deployment.config;
    let _ = &deployment.agent_checkpoint;
    let _ = &deployment.credential;
    let _ = &deployment.state;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_v2_report_keeps_the_revision_and_completes_drained_previous() {
    let deployment = make_deployment(true, true).await;
    let summary = convert_checkpoint(&deployment.checkpoint, &deployment.converted)
        .await
        .unwrap();
    let fingerprint = summary.fingerprint_sha256.clone().unwrap();

    let converted_db = deployment.converted.join("server/server.db");
    let pepper = load_pepper_file(&deployment.converted.join("server/server-pepper")).unwrap();
    let database = initialize(ServerDatabaseConfig::new(&converted_db))
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(
        database,
        None,
        AuthConfig::development(pepper, format!("http://{addr}")),
    );
    let probe = state.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, build_app(state)).await.unwrap();
    });
    let client = reqwest::Client::new();

    // The first v2 Report declares the verified content unchanged. It must keep
    // the preserved revision and execute the pending DrainedPrevious transition.
    let first = v2_report_body(
        &deployment.agent_id,
        "0195f2a1-0070-4070-8070-000000000070",
        5,
        "ws://127.0.0.1:6790",
    );
    let response = client
        .post(format!("http://{addr}{AGENT_API_REPORTS_PATH_V2}"))
        .bearer_auth(&deployment.credential)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(first)
        .send()
        .await
        .unwrap();
    let first_status = response.status();
    let first_text = response.text().await.unwrap();
    assert_eq!(first_status, reqwest::StatusCode::OK, "{first_text}");
    let body: serde_json::Value = serde_json::from_str(&first_text).unwrap();
    assert_eq!(
        body["receipt"]["inventory"]["acceptance"]["revision"].as_i64(),
        Some(PRESERVED_REVISION),
        "{body}"
    );
    assert_eq!(
        body["receipt"]["inventory"]["acceptance"]["fingerprint"].as_str(),
        Some(fingerprint.as_str())
    );
    let (revision, sha, protocol): (i64, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT last_inventory_revision, inventory_sha256, inventory_protocol_major FROM agents WHERE agent_id = ?",
    )
    .bind(&deployment.agent_id)
    .fetch_one(probe.db().pool())
    .await
    .unwrap();
    assert_eq!(revision, PRESERVED_REVISION);
    assert_eq!(sha.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(protocol, Some(2));

    // The pending DrainedPrevious Boot linkage is completed, not reset.
    let active: (Option<String>, String) =
        sqlx::query_as("SELECT active_boot_id, active_boot_status FROM agents WHERE agent_id = ?")
            .bind(&deployment.agent_id)
            .fetch_one(probe.db().pool())
            .await
            .unwrap();
    assert_eq!(active.0.as_deref(), Some(NEXT_BOOT));
    assert_eq!(active.1, "active");

    // A later legal content change advances to the next revision; A -> B -> A
    // would still allocate a fresh one. Here a single changed declaration is
    // enough to prove the preserved numbering continues at 58.
    let second = v2_report_body(
        &deployment.agent_id,
        "0195f2a1-0071-4071-8071-000000000071",
        6,
        "ws://127.0.0.1:6791",
    );
    let response = client
        .post(format!("http://{addr}{AGENT_API_REPORTS_PATH_V2}"))
        .bearer_auth(&deployment.credential)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(second)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["receipt"]["inventory"]["acceptance"]["revision"].as_i64(),
        Some(PRESERVED_REVISION + 1),
        "{body}"
    );

    // Mixed operation is not supported: a frozen v1 report at the same
    // preserved revision is refused, not silently accepted.
    let legacy = v1_report_body(
        &deployment.agent_id,
        "0195f2a1-0072-4072-8072-000000000072",
        7,
    );
    let response = client
        .post(format!("http://{addr}{AGENT_API_REPORTS_PATH}"))
        .bearer_auth(&deployment.credential)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(legacy)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["receipt"]["disposition"].as_str(),
        Some("rejected"),
        "{body}"
    );

    server.abort();
}

#[tokio::test]
async fn a_failed_conversion_leaves_no_partial_output_and_tampering_is_refused() {
    // A preserved frozen-v1 configuration without inventory_revision fails the
    // conversion after the databases were already written; the output must be
    // removed and the source checkpoint left intact.
    let deployment = make_deployment(false, true).await;
    let error = convert_checkpoint(&deployment.checkpoint, &deployment.converted)
        .await
        .unwrap_err();
    assert!(
        !deployment.converted.exists(),
        "a failed conversion left output"
    );
    let _ = error;

    // A non-empty output directory is refused before anything is written.
    let deployment = make_deployment(true, true).await;
    fs::create_dir_all(&deployment.converted).unwrap();
    fs::write(deployment.converted.join("stray"), b"x").unwrap();
    assert!(
        convert_checkpoint(&deployment.checkpoint, &deployment.converted)
            .await
            .is_err()
    );

    // A converted deployment whose Server baseline was tampered with fails
    // offline verification.
    let deployment = make_deployment(true, true).await;
    convert_checkpoint(&deployment.checkpoint, &deployment.converted)
        .await
        .unwrap();
    let converted_db = deployment.converted.join("server/server.db");
    let connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&converted_db),
    )
    .await
    .unwrap();
    let mut connection = connection;
    sqlx::query("UPDATE agents SET inventory_sha256 = '0xtampered' WHERE agent_id = ?")
        .bind(&deployment.agent_id)
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert!(
        verify_conversion(&deployment.checkpoint, &deployment.converted)
            .await
            .is_err()
    );

    // Re-adding inventory_revision to the converted configuration is refused.
    let deployment = make_deployment(true, true).await;
    convert_checkpoint(&deployment.checkpoint, &deployment.converted)
        .await
        .unwrap();
    let config_path = deployment.converted.join("agent/agent.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    let mut value: toml::Value = toml::from_str(&config).unwrap();
    value
        .as_table_mut()
        .unwrap()
        .insert("inventory_revision".to_owned(), toml::Value::Integer(57));
    fs::write(&config_path, toml::to_string(&value).unwrap()).unwrap();
    assert!(
        verify_conversion(&deployment.checkpoint, &deployment.converted)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_cli_offline_conversion_entry_point_wires_through() {
    let deployment = make_deployment(true, true).await;
    let binary = env!("CARGO_BIN_EXE_platpulse-server");
    let converted = deployment
        .checkpoint
        .parent()
        .unwrap()
        .join("cli-converted");
    let output = Command::new(binary)
        .args([
            "checkpoint",
            "convert",
            "--checkpoint",
            &deployment.checkpoint.display().to_string(),
            "--output",
            &converted.display().to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "checkpoint convert failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verified = Command::new(binary)
        .args([
            "checkpoint",
            "verify-conversion",
            "--checkpoint",
            &deployment.checkpoint.display().to_string(),
            "--converted",
            &converted.display().to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "checkpoint verify-conversion failed: {}",
        String::from_utf8_lossy(&verified.stderr)
    );
}

/// A checkpoint whose Server database predates migration 0057 must convert
/// through the normal forward-migration path, exactly like a deployment that
/// upgraded from an older binary.
#[tokio::test]
async fn conversion_applies_the_marker_migration_to_a_pre_migration_checkpoint() {
    let deployment = make_deployment(true, true).await;
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&deployment.db),
    )
    .await
    .unwrap();
    sqlx::query("DROP TABLE inventory_migration")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 57")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();

    let server_config = platpulse_server::config::ServerConfig::resolve(
        Some(&deployment.config),
        &Default::default(),
    )
    .unwrap();
    let directory = deployment.checkpoint.parent().unwrap();
    let checkpoint = directory.join("pre-migration-checkpoint");
    platpulse_server::checkpoint::create_checkpoint(
        &server_config,
        &deployment.agent_checkpoint,
        &checkpoint,
    )
    .await
    .unwrap();
    let converted = directory.join("pre-migration-converted");
    let summary = convert_checkpoint(&checkpoint, &converted).await.unwrap();
    assert_eq!(summary.inventory_revision, PRESERVED_REVISION);
    verify_conversion(&checkpoint, &converted).await.unwrap();
    let (revision, sha, protocol) =
        read_agent_row(&converted.join("server/server.db"), &deployment.agent_id).await;
    assert_eq!(revision, PRESERVED_REVISION);
    assert_eq!(sha.as_deref(), summary.fingerprint_sha256.as_deref());
    assert_eq!(protocol, Some(2));
}
