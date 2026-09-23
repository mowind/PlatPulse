#![cfg(unix)]
//! Subprocess coverage for the Agent half of the coordinated checkpoint
//! (issue #190): the real binary refuses while another runtime owns the Store,
//! before SQLite opens, and leaves nothing behind. Once the Store is released
//! it reaches the Store and refuses only because no verified preparation
//! exists, still without leaving a partial checkpoint directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use platpulse_agent::AgentRuntimeLock;
use tempfile::TempDir;

fn write_config(dir: &Path) -> PathBuf {
    let path = dir.join("agent.toml");
    fs::write(
        &path,
        format!(
            "server_url=\"https://example.com\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
            dir.join("credential").display(),
            dir.join("agent.db").display(),
        ),
    )
    .unwrap();
    path
}

fn run(config: &Path, output: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_platpulse-agent"))
        .args(["checkpoint", "create", "--config"])
        .arg(config)
        .arg("--output")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

#[test]
fn checkpoint_create_refuses_while_the_runtime_owns_the_store() {
    let dir = TempDir::new().unwrap();
    let config = write_config(dir.path());
    let output = dir.path().join("checkpoint");
    let store = dir.path().join("agent.db");

    let held = AgentRuntimeLock::acquire(&store).unwrap();
    let refused = run(&config, &output);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("Agent runtime owns this Store"),
        "unexpected refusal: {stderr}"
    );
    assert!(!output.exists(), "refusal created output");
    assert!(!store.exists(), "refusal opened (created) the Agent Store");
    drop(held);

    // The runtime released the Store: the command now reaches it and refuses
    // only because no verified preparation exists. No partial directory stays.
    let unprepared = run(&config, &output);
    assert!(!unprepared.status.success());
    let stderr = String::from_utf8_lossy(&unprepared.stderr);
    assert!(
        stderr.contains("prepare-upgrade")
            || stderr.contains("no verified upgrade preparation")
            || stderr.contains("Agent Store is not enrolled"),
        "unexpected refusal: {stderr}"
    );
    assert!(!output.exists(), "a failed checkpoint left output behind");
    assert!(store.exists(), "the Agent Store should have been created");
}

/// The real binary's success path, end to end: an in-process v1 Server accepts
/// the Agent's preparation Closing, and then the operator entry point preserves
/// the prepared checkpoint.
#[tokio::test]
async fn checkpoint_create_succeeds_after_a_real_preparation() {
    use platpulse_agent::config::AgentConfig;
    use platpulse_server::auth::{AuthConfig, create_owner, hash_password};
    use platpulse_server::database::{ServerDatabaseConfig, initialize};
    use platpulse_server::enrollment::{
        ENROLLMENT_TOKEN_DEFAULT_LIFETIME, create_enrollment_token,
    };
    use platpulse_server::http::{AppState, build_app};
    use platpulse_server::network::create_network;
    use platpulse_server::secrets::{create_pepper_file, load_pepper_file};
    use sqlx::Connection;
    use tokio_util::sync::CancellationToken;

    let dir = TempDir::new().unwrap();
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    // In-process frozen-v1 Server.
    let database = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
        .await
        .unwrap();
    let pepper_path = dir.path().join("server-pepper");
    create_pepper_file(&pepper_path).unwrap();
    let pepper = load_pepper_file(&pepper_path).unwrap();
    let auth = AuthConfig::development(pepper, format!("http://{addr}"));
    let token =
        create_enrollment_token(&database, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap()
            .token;
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
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let state = AppState::new(database, None, auth);
    let shutdown = CancellationToken::new();
    let served = state.clone();
    let stopping = shutdown.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, build_app(served))
            .with_graceful_shutdown(async move { stopping.cancelled().await })
            .await
            .unwrap();
    });

    let config_path = dir.path().join("agent.toml");
    fs::write(
        &config_path,
        format!(
            "server_url=\"http://{addr}\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
            dir.path().join("credential").display(),
            dir.path().join("agent.db").display(),
        ),
    )
    .unwrap();
    let config = AgentConfig::resolve(&config_path).unwrap();
    platpulse_agent::enroll::enroll_agent(&config, &token)
        .await
        .unwrap();
    {
        // Enrollment leaves the fresh Store without a Boot identity; give it
        // one so the preparation bridge can persist a Closing.
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&config.state_db);
        let mut connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_state SET boot_id=?, boot_state='active' WHERE singleton=1")
            .bind("0195f2a1-0012-4012-8012-000000000012")
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
    }
    platpulse_agent::preparation::prepare_v1_upgrade(&config, std::time::Duration::from_secs(5))
        .await
        .unwrap();

    let output = dir.path().join("checkpoint");
    let result = run(&config_path, &output);
    assert!(
        result.status.success(),
        "checkpoint create failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.join("agent-checkpoint.json").is_file());
    assert!(output.join("agent-store.sqlite").is_file());

    shutdown.cancel();
    let _ = task.await;
    state.db().close().await;
}
