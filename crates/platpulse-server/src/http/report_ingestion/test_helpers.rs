//! Fixtures and submission helpers shared by the Report Ingestion tests.
use super::*;

use std::sync::Arc;

use axum::body::{Bytes, to_bytes};
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use tempfile::TempDir;

use crate::auth::{AuthConfig, format_rfc3339};
use crate::enrollment::AgentAuthInfo;
use crate::http::{AppState, RequestId};
use crate::network::create_network;
use crate::secrets::{create_pepper_file, load_pepper_file};
use platpulse_core::ReportReceipt;

pub(super) async fn state_with_agent() -> (TempDir, AppState, String) {
    let dir = TempDir::new().unwrap();
    let database = crate::test_support::test_database_in(dir.path()).await;
    let pepper_path = dir.path().join("pepper");
    create_pepper_file(&pepper_path).unwrap();
    let auth = AuthConfig::development(
        load_pepper_file(&pepper_path).unwrap(),
        "http://127.0.0.1:8080".to_owned(),
    );
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
    let agent_id = "0195f2a1-0011-4011-8011-000000000011".to_owned();
    sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)").bind(&agent_id).bind("2026-08-12T08:00:00Z").bind("2026-08-12T08:00:00Z").execute(database.pool()).await.unwrap();
    (dir, AppState::new(database, None, auth), agent_id)
}

/// Reopen the same database file the way a restarted Server does: the same
/// schema, the same rows, and nothing carried over in process memory.
pub(super) async fn restart(dir: &TempDir) -> AppState {
    let database = crate::test_support::test_database_in(dir.path()).await;
    let pepper_path = dir.path().join("pepper");
    let auth = AuthConfig::development(
        load_pepper_file(&pepper_path).unwrap(),
        "http://127.0.0.1:8080".to_owned(),
    );
    AppState::new(database, None, auth)
}

pub(super) async fn submit(state: &AppState, agent_id: &str, body: Vec<u8>) -> ReportReceipt {
    let response = handler(
        State(state.clone()),
        Extension(AgentAuthInfo {
            agent_id: agent_id.to_owned(),
            credential_id: "test-credential".to_owned(),
        }),
        Extension(RequestId(Arc::from("test-request"))),
        Bytes::from(body),
    )
    .await;
    if response.status() != StatusCode::OK {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        panic!(
            "unexpected report response for agent {agent_id}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice::<ReportResponse>(&body)
        .unwrap()
        .receipt
}

/// The instant the report fixture is written around.
const FIXTURE_INSTANT: &str = "2026-08-12T09:00:00Z";

/// Move every instant in a report fixture onto the Server clock, keeping the
/// fixture's own relative offsets.
///
/// Raw metric samples are kept for the 24 hours of the `raw_metric_sample`
/// policy (issue #213), so a fixture stamped in August is released the moment
/// it is stored: a case about an accepted observation has to deliver one the
/// raw window can still hold.
pub(super) fn restamp_fixture(raw: &mut serde_json::Value, now: time::OffsetDateTime) {
    fn walk(value: &mut serde_json::Value, delta: time::Duration) {
        match value {
            serde_json::Value::String(text) => {
                if let Some(instant) = crate::auth::parse_rfc3339(text) {
                    *text = format_rfc3339(instant + delta);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items.iter_mut() {
                    walk(item, delta);
                }
            }
            serde_json::Value::Object(fields) => {
                for field in fields.values_mut() {
                    walk(field, delta);
                }
            }
            _ => {}
        }
    }
    let anchor = crate::auth::parse_rfc3339(FIXTURE_INSTANT).expect("the fixture anchor parses");
    walk(raw, now - anchor);
}

pub(super) async fn submit_status(state: &AppState, agent_id: &str, body: Vec<u8>) -> StatusCode {
    handler(
        State(state.clone()),
        Extension(AgentAuthInfo {
            agent_id: agent_id.to_owned(),
            credential_id: "test-credential".to_owned(),
        }),
        Extension(RequestId(Arc::from("test-request"))),
        Bytes::from(body),
    )
    .await
    .status()
}

pub(super) async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// One Node exactly as the Home list route renders it, so the assertion
/// proves the Public Projection the Home cards read.
pub(super) async fn public_home_node(state: &AppState, node_id: &str) -> serde_json::Value {
    let response = crate::http::public::public_networks(
        State(state.clone()),
        axum::Extension(crate::http::RequestId::for_test()),
    )
    .await;
    let body = body_json(response).await;
    body.as_array()
        .unwrap()
        .iter()
        .flat_map(|network| network["nodes"].as_array().unwrap())
        .find(|node| node["nodeId"] == node_id)
        .cloned()
        .expect("the Node is in the Public Home list")
}

pub(super) fn peer_snapshot_body(peers: serde_json::Value) -> serde_json::Value {
    let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    value["nodes"][0]["chain"]["peers"] = serde_json::json!({
        "status": "ok",
        "attempted_at": "2026-08-12T10:00:00Z",
        "latest_observed_at": "2026-08-12T10:00:00Z",
        "state_revision": 1,
        "value_revision": 1,
        "latest": {"peers": peers}
    });
    value
}

pub(super) fn at(base: time::OffsetDateTime, seconds: i64) -> String {
    format_rfc3339(base + time::Duration::seconds(seconds))
}
