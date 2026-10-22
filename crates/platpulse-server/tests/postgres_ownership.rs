//! Ownership acceptance tests for the PostgreSQL Storage Backend (issue #255):
//! a second Server must fail closed, a severed ownership connection must be
//! observed instead of reacquired, and the password must never surface in an
//! error. The tests skip when no PostgreSQL test server is configured.

use std::time::Duration;

use platpulse_server::database::{
    PostgresConnectConfig, PostgresHost, PostgresPassword, PostgresSslMode, ServerDatabase,
    StorageConfig,
};
use platpulse_server::test_support::{self, PostgresDeployment};

/// The heartbeat a test uses so ownership loss is noticed quickly.
const TEST_HEARTBEAT: Duration = Duration::from_millis(200);

fn connect_config(deployment: &PostgresDeployment) -> PostgresConnectConfig {
    let password = PostgresPassword::new(deployment.password())
        .expect("the PostgreSQL test deployment carries a password");
    PostgresConnectConfig::new(
        PostgresHost::Tcp(deployment.host().to_owned()),
        deployment.port(),
        deployment.database(),
        deployment.user(),
        password,
    )
    .with_ssl_mode(PostgresSslMode::Disable)
    .with_connect_timeout(Duration::from_secs(10))
    .with_ownership_heartbeat(TEST_HEARTBEAT)
}

async fn open(deployment: &PostgresDeployment) -> ServerDatabase {
    ServerDatabase::open_existing(StorageConfig::Postgres(connect_config(deployment)))
        .await
        .expect("the Server opens the PostgreSQL test database")
}

#[tokio::test]
async fn a_second_server_cannot_own_one_database() {
    let Some(deployment) = test_support::postgres_test_deployment().await else {
        eprintln!("skipping: PLATPULSE_TEST_POSTGRES_URL is not set");
        return;
    };
    let first = open(&deployment).await;
    assert!(first.ownership_held(), "the first Server owns the database");

    let second =
        ServerDatabase::open_existing(StorageConfig::Postgres(connect_config(&deployment))).await;
    let error = match second {
        Ok(_) => panic!("a second Server must not serve a database another Server owns"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("another Server owns this PostgreSQL database"),
        "unexpected refusal: {error}"
    );

    first.close().await;
    let third = open(&deployment).await;
    assert!(
        third.ownership_held(),
        "closing the first Server releases the advisory lock for the next one"
    );
    third.close().await;
}

#[tokio::test]
async fn losing_the_ownership_connection_is_observed_and_never_reacquired() {
    let Some(deployment) = test_support::postgres_test_deployment().await else {
        eprintln!("skipping: PLATPULSE_TEST_POSTGRES_URL is not set");
        return;
    };
    let database = open(&deployment).await;
    assert!(database.ownership_held(), "the Server owns the database");

    deployment.terminate_backends().await;

    tokio::time::timeout(Duration::from_secs(20), database.ownership_lost())
        .await
        .expect("the Server notices its severed ownership connection");
    assert!(
        !database.ownership_held(),
        "ownership is lost after the ownership connection is severed"
    );

    // Ownership is never reacquired in process: the next work statement fails
    // instead of quietly reconnecting.
    let work = sqlx::query("SELECT 1").execute(database.pool()).await;
    assert!(
        work.is_err(),
        "a lost ownership connection never yields a working pool"
    );
    database.close().await;
}

#[tokio::test]
async fn a_failed_connection_never_prints_the_password() {
    let password = PostgresPassword::new("s3cret-never-logged").expect("a password");
    let config = PostgresConnectConfig::new(
        PostgresHost::Tcp("127.0.0.1".to_owned()),
        1,
        "platpulse",
        "platpulse",
        password,
    )
    .with_ssl_mode(PostgresSslMode::Disable)
    .with_connect_timeout(Duration::from_secs(2));

    let error = match ServerDatabase::open_existing(StorageConfig::Postgres(config)).await {
        Ok(database) => {
            database.close().await;
            panic!("port 1 has no PostgreSQL server");
        }
        Err(error) => error.to_string(),
    };
    assert!(
        !error.contains("s3cret-never-logged"),
        "the connection error must never carry the password: {error}"
    );
}
