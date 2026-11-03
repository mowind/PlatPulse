#![cfg(unix)]
//! Subprocess coverage for how the real `platpulse-agent run` answers a stop
//! signal. The Agent installs its SIGTERM and SIGINT handlers before it logs
//! `starting`, so a stop that arrives while it is still starting (opening the
//! Agent Store, recovering the previous Boot, spawning the Node workers) is
//! held and answered by the same graceful shutdown once startup finishes. It
//! never falls through to the default disposition that kills the process
//! without a Closing report and loses the queued log lines.

#![allow(
    clippy::unwrap_used,
    reason = "helpers outside #[test] functions unwrap test fixtures"
)]

use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use platpulse_agent::config::AgentConfig;
use platpulse_server::auth::{AuthConfig, create_owner, hash_password};
use platpulse_server::database::{ServerDatabaseConfig, initialize};
use platpulse_server::enrollment::{ENROLLMENT_TOKEN_DEFAULT_LIFETIME, create_enrollment_token};
use platpulse_server::http::{AppState, build_app};
use platpulse_server::network::create_network;
use platpulse_server::secrets::{create_pepper_file, load_pepper_file};
use sqlx::Connection;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_signalled_while_starting_still_shuts_down_gracefully() {
    let dir = TempDir::new().unwrap();
    // The listener is bound once and handed to the Server, so no other test
    // can take the port in between.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // A Node endpoint nobody listens on: the Node workers keep retrying it
    // and never end the run on their own.
    let node_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

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
    let state = AppState::new(database, None, auth);
    let shutdown = CancellationToken::new();
    let served = state.clone();
    let stopping = shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, build_app(served))
            .with_graceful_shutdown(async move { stopping.cancelled().await })
            .await
            .unwrap();
    });

    let config_path = dir.path().join("agent.toml");
    fs::write(
        &config_path,
        format!(
            "server_url=\"http://{addr}\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:{node_port}\"}}]\n",
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
        // Give the enrolled Store a Boot, so the run has a previous Boot to
        // recover during startup and a Boot to close on shutdown.
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

    let stderr_path = dir.path().join("run.stderr");
    let mut agent = Command::new(env!("CARGO_BIN_EXE_platpulse-agent"))
        .args(["--log-format", "json", "run", "--config"])
        .arg(&config_path)
        .args([
            "--drain-deadline-ms",
            "2000",
            "--sender-deadline-ms",
            "5000",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(&stderr_path).unwrap()))
        .spawn()
        .unwrap();

    // The first lifecycle line is the earliest point the process says it is
    // the Agent; signal it there, before it has opened the Store.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !fs::read_to_string(&stderr_path)
        .unwrap_or_default()
        .contains("starting")
    {
        assert!(Instant::now() < deadline, "Agent never logged starting");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let signalled = Command::new("kill")
        .args(["-TERM", &agent.id().to_string()])
        .status()
        .unwrap();
    assert!(signalled.success());
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = agent.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "Agent did not stop on SIGTERM");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let stderr = fs::read_to_string(&stderr_path).unwrap();

    shutdown.cancel();
    let _ = server.await;
    state.db().close().await;

    assert_eq!(
        status.signal(),
        None,
        "the startup SIGTERM killed the Agent instead of stopping it: {stderr}"
    );
    assert!(status.success(), "graceful shutdown failed: {stderr}");
    assert!(stderr.contains("Agent stopped"), "{stderr}");
    assert!(stderr.contains("\"outcome\":\"final_stored\""), "{stderr}");
}
