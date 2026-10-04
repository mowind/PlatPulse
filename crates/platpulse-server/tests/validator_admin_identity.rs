//! Owner-only automatic Validator identity acceptance through the real router
//! (issue #218, part of #202 stage 3).
//!
//! The Admin surface must expose the automatic identity outcome for every Node
//! (identification, conflict, missing evidence, and a Node the discovery
//! dimension has never examined), the Current Validator Status of every
//! Validator without turning a Provider failure into a fresh negative, and the
//! Node-to-Validator entry itself. These tests build the full application with
//! build_app against a temporary SQLite database, drive the real discovery
//! pass, and read the results back over HTTP.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_server::{AppState, auth, database, http, network, secrets, validator};

const NETWORK_KEY: &str = "platon-mainnet";
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const NETWORK_CHAIN_ID: i64 = 210425;
const NETWORK_P2P_ID: i64 = 210425;
const NETWORK_HRP: &str = "lat";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer","password":"viewer password"}"#;
const AGENT_ID: &str = "0195f2a1-0018-4018-8018-000000000018";
const NODE_IDENTIFIED: &str = "0195f2a1-0018-4018-8018-000000000101";
const NODE_MISSING_KEY: &str = "0195f2a1-0018-4018-8018-000000000102";
const NODE_MISMATCHED: &str = "0195f2a1-0018-4018-8018-000000000103";
const NODE_NO_IDENTITY: &str = "0195f2a1-0018-4018-8018-000000000104";
const NODE_UNEVALUATED: &str = "0195f2a1-0018-4018-8018-000000000105";

/// A fresh Server (real temp SQLite + pepper), the full router, and the pieces
/// needed to authenticate as the Owner.
struct Harness {
    /// Kept alive for the lifetime of the harness so the temporary database
    /// directory is removed when the test finishes.
    _dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
    async fn boot() -> Self {
        let dir = TempDir::new().unwrap();
        Self::seed(dir).await
    }

    async fn seed(dir: TempDir) -> Self {
        let harness = Self::open(dir).await;
        let owner_hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(harness.state.db(), "admin", &owner_hash)
            .await
            .unwrap();
        let viewer_hash = auth::hash_password(b"viewer password").unwrap();
        auth::create_viewer(harness.state.db(), "viewer", &viewer_hash)
            .await
            .unwrap();
        network::create_network(
            harness.state.db(),
            NETWORK_KEY,
            "PlatON Mainnet",
            NETWORK_GENESIS,
            NETWORK_CHAIN_ID as u64,
            NETWORK_P2P_ID as u64,
            NETWORK_HRP,
        )
        .await
        .unwrap();
        harness.seed_agent().await;
        harness
    }

    async fn open(dir: TempDir) -> Self {
        let database = database::initialize(database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("server-pepper");
        if !pepper_path.exists() {
            secrets::create_pepper_file(&pepper_path).unwrap();
        }
        let auth = auth::AuthConfig::development(
            secrets::load_pepper_file(&pepper_path).unwrap(),
            DEVELOPMENT_ORIGIN.to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let app = http::build_app(state.clone());
        Self {
            _dir: dir,
            state,
            app,
        }
    }

    /// One Agent row so Nodes can reference it; the identity projection never
    /// reads Agent data, but the Nodes table requires the reference.
    async fn seed_agent(&self) {
        let now = self.now("0 seconds").await;
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at, last_received_at, shutdown_state, security_event_count) VALUES (?, 1, ?, ?, ?, 'running', 0)")
            .bind(AGENT_ID)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// An Active Node on the registered Network.
    async fn seed_node(&self, node_id: &str, display_name: &str) {
        let now = self.now("0 seconds").await;
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, ?, ?, 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(node_id)
            .bind(AGENT_ID)
            .bind(NETWORK_KEY)
            .bind(display_name)
            .bind(&now)
            .bind(&now)
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// A chain observation row as an Agent report would leave it: an optional
    /// Network Identity, an optional full P2P key (enode), and nothing else.
    async fn seed_chain_observation(
        &self,
        node_id: &str,
        chain_id: Option<i64>,
        enode: Option<&str>,
    ) {
        let now = self.now("0 seconds").await;
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, enode, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(node_id)
            .bind(NETWORK_GENESIS)
            .bind(chain_id)
            .bind(NETWORK_P2P_ID)
            .bind(NETWORK_HRP)
            .bind(enode)
            .bind(&now)
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// A Validator row plus one Provider insight row, as the refresh pass would
    /// leave them. `last_good_modifier` is a relative SQLite
    /// modifier, so the freshness window is exercised against the real clock.
    async fn seed_validator(
        &self,
        validator_id: &str,
        node_key_digit: char,
        outcome: &str,
        activity: Option<&str>,
        last_good_modifier: Option<&str>,
    ) {
        let now = self.now("0 seconds").await;
        let last_good = match last_good_modifier {
            Some(modifier) => Some(self.now(modifier).await),
            None => None,
        };
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (?, ?, ?, NULL, ?, ?)")
            .bind(validator_id)
            .bind(NETWORK_KEY)
            .bind(p2p_key(node_key_digit))
            .bind(&now)
            .bind(&now)
            .execute(self.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, counter_state, updated_at) VALUES (?, 'platscan', ?, NULL, NULL, ?, ?, ?, 'normal', ?)")
            .bind(validator_id)
            .bind(outcome)
            .bind(activity)
            .bind(&now)
            .bind(last_good.as_deref())
            .bind(&now)
            .execute(self.pool())
            .await
            .unwrap();
    }

    fn pool(&self) -> &SqlitePool {
        self.state.db().pool()
    }

    /// The Server clock, read from the same database the Server uses, so a
    /// relative modifier produces a timestamp the freshness window compares.
    async fn now(&self, modifier: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now',?)")
            .bind(modifier)
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    async fn send(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }
}

/// A full-length P2P public key ending in `digit`, so two
/// Nodes never share a key by accident.
fn p2p_key(digit: char) -> String {
    format!("0x{}{}", "0".repeat(127), digit)
}

fn enode(digit: char) -> String {
    format!("enode://{}", &p2p_key(digit)[2..])
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

struct Session {
    cookie: String,
    csrf: String,
}

fn admin_get(uri: &str, session: &Session) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, &session.cookie)
        .body(Body::empty())
        .unwrap()
}

fn anonymous_get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn admin_post(uri: &str, session: &Session, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .header("x-csrf-token", &session.csrf)
        .header(header::COOKIE, &session.cookie)
        .body(Body::from(body.to_owned()))
        .unwrap()
}

async fn login(harness: &Harness, body: &str) -> Session {
    let request = Request::builder()
        .method("POST")
        .uri("/api/public/v1/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    let value = body_json(response).await;
    Session {
        cookie,
        csrf: value["csrfToken"].as_str().unwrap().to_owned(),
    }
}

async fn identities(harness: &Harness, session: &Session) -> Vec<Value> {
    let response = harness
        .send(admin_get("/api/admin/v1/validator-identities", session))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await.as_array().cloned().unwrap()
}

fn identity_of<'a>(identities: &'a [Value], node_id: &str) -> &'a Value {
    identities
        .iter()
        .find(|identity| identity["nodeId"] == node_id)
        .unwrap_or_else(|| panic!("no identity entry for {node_id}"))
}

/// The identification, conflict, missing-evidence, and never-evaluated states
/// are all visible to the Owner, and a Node the discovery pass has not examined
/// yet reports as prospective coverage rather than as an absence of staking.
#[tokio::test]
async fn automatic_identity_states_are_visible_for_identification_conflict_and_absence() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;

    harness.seed_node(NODE_IDENTIFIED, "Identified").await;
    harness
        .seed_chain_observation(NODE_IDENTIFIED, Some(NETWORK_CHAIN_ID), Some(&enode('a')))
        .await;
    harness.seed_node(NODE_MISSING_KEY, "Missing key").await;
    harness
        .seed_chain_observation(NODE_MISSING_KEY, Some(NETWORK_CHAIN_ID), None)
        .await;
    harness.seed_node(NODE_MISMATCHED, "Mismatched").await;
    harness
        .seed_chain_observation(NODE_MISMATCHED, Some(999_999), Some(&enode('b')))
        .await;
    harness.seed_node(NODE_NO_IDENTITY, "No identity").await;

    let summary = validator::discover_automatic_links(harness.state.db())
        .await
        .unwrap();
    assert_eq!(summary.considered, 4);
    assert_eq!(summary.identified, 1);
    assert_eq!(summary.newly_linked, 1);

    // A Node that appears after the pass has never been evaluated: its state is
    // explicitly prospective, never an inferred absence.
    harness.seed_node(NODE_UNEVALUATED, "Later").await;

    let identities = identities(&harness, &owner).await;
    assert_eq!(identities.len(), 5);

    let identified = identity_of(&identities, NODE_IDENTIFIED);
    assert_eq!(identified["state"], "identified");
    assert_eq!(identified["reason"], Value::Null);
    assert_eq!(identified["observedValidatorNodeKey"], p2p_key('a'));
    assert_eq!(identified["validatorNodeKey"], p2p_key('a'));
    assert_eq!(identified["networkKey"], NETWORK_KEY);
    assert_eq!(identified["lifecycle"], "active");
    assert_eq!(identified["associationEffective"], true);
    assert!(identified["validatorId"].is_string());
    assert!(identified["evaluatedAt"].is_string());

    let missing_key = identity_of(&identities, NODE_MISSING_KEY);
    assert_eq!(missing_key["state"], "missing_public_key");
    assert_eq!(
        missing_key["reason"],
        "No full P2P public key has been observed for this Node, so no Validator can be identified."
    );
    assert_eq!(missing_key["validatorId"], Value::Null);
    assert_eq!(missing_key["associationEffective"], false);

    let mismatched = identity_of(&identities, NODE_MISMATCHED);
    assert_eq!(mismatched["state"], "network_identity_mismatch");
    assert_eq!(
        mismatched["reason"],
        "The observed Network Identity does not match this Node's registered Network; no cross-Network Validator was searched."
    );
    assert_eq!(mismatched["validatorId"], Value::Null);
    assert_eq!(mismatched["observedValidatorNodeKey"], Value::Null);

    let no_identity = identity_of(&identities, NODE_NO_IDENTITY);
    assert_eq!(no_identity["state"], "network_identity_missing");
    assert_eq!(
        no_identity["reason"],
        "No Network Identity has been observed for this Node, so no Validator can be identified."
    );

    let unevaluated = identity_of(&identities, NODE_UNEVALUATED);
    assert_eq!(unevaluated["state"], "not_evaluated");
    assert_eq!(
        unevaluated["reason"],
        "No automatic Validator identity evaluation has been recorded for this Node yet."
    );
    assert_eq!(unevaluated["evaluatedAt"], Value::Null);

    // The Node detail carries the same block, so the Node entry point and the
    // coverage list can never disagree about the same Node.
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/nodes/{NODE_IDENTIFIED}"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let detail = body_json(response).await;
    assert_eq!(detail["validator_identity"]["state"], "identified");
    assert_eq!(
        detail["validator_identity"]["validatorNodeKey"],
        p2p_key('a')
    );
    assert_eq!(detail["validator_identity"]["associationEffective"], true);

    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/nodes/{NODE_MISMATCHED}"),
            &owner,
        ))
        .await;
    let detail = body_json(response).await;
    assert_eq!(
        detail["validator_identity"]["state"],
        "network_identity_mismatch"
    );
    assert_eq!(detail["validator_identity"]["validatorId"], Value::Null);
}

/// The Current Validator Status projection keeps a retained exit visible while
/// stale, and never lets a Provider failure become a fresh negative or a zero.
#[tokio::test]
async fn current_validator_status_never_turns_provider_failure_into_a_negative() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;

    harness
        .seed_validator(
            "v-fresh-exited",
            '1',
            "success",
            Some("exited"),
            Some("0 seconds"),
        )
        .await;
    harness
        .seed_validator(
            "v-stale-exited",
            '2',
            "error",
            Some("exited"),
            Some("-1 hour"),
        )
        .await;
    harness
        .seed_validator("v-error-no-last-good", '3', "error", None, None)
        .await;
    harness
        .seed_validator(
            "v-authoritative-absence",
            '4',
            "empty",
            None,
            Some("0 seconds"),
        )
        .await;
    harness
        .seed_validator("v-not-configured", '5', "not_configured", None, None)
        .await;
    harness
        .seed_validator(
            "v-locked",
            '6',
            "success",
            Some("locked"),
            Some("0 seconds"),
        )
        .await;
    harness
        .seed_validator(
            "v-verifying",
            '7',
            "success",
            Some("verifying"),
            Some("-2 hours"),
        )
        .await;

    let response = harness
        .send(admin_get("/api/admin/v1/validators", &owner))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let listed = body_json(response).await;
    let validators = listed.as_array().cloned().unwrap();
    assert_eq!(validators.len(), 7);
    let insight_of = |validator_id: &str| -> Value {
        validators
            .iter()
            .find(|entry| entry["validatorId"] == validator_id)
            .unwrap_or_else(|| panic!("no Validator entry for {validator_id}"))["insight"]
            .clone()
    };

    let fresh_exit = insight_of("v-fresh-exited");
    assert_eq!(fresh_exit["currentValidatorStatus"], "not_validator");
    assert_eq!(fresh_exit["currentValidatorStatusState"], "current");
    assert_eq!(fresh_exit["currentValidatorStatusQualifier"], Value::Null);
    assert_eq!(fresh_exit["activity"], "exited");
    assert_eq!(fresh_exit["activityState"], "current");
    let age = fresh_exit["lastGoodAgeSeconds"].as_i64().unwrap();
    assert!((0..300).contains(&age), "unexpected fresh age {age}");

    // A failed attempt keeps the retained verdict and its age, marked stale.
    let stale_exit = insight_of("v-stale-exited");
    assert_eq!(stale_exit["currentValidatorStatus"], "not_validator");
    assert_eq!(stale_exit["currentValidatorStatusState"], "stale");
    assert_eq!(stale_exit["activity"], "exited");
    assert_eq!(stale_exit["activityState"], "stale");
    let age = stale_exit["lastGoodAgeSeconds"].as_i64().unwrap();
    assert!(age >= 3_540, "unexpected stale age {age}");

    // A failure with no retained verdict is Unknown, never Not Validator, and
    // never a fresh zero.
    let unknown = insight_of("v-error-no-last-good");
    assert_eq!(unknown["currentValidatorStatus"], "unknown");
    assert_eq!(unknown["currentValidatorStatusState"], "unknown");
    assert_eq!(unknown["activityState"], "unknown");
    assert_eq!(unknown["lastGoodAgeSeconds"], Value::Null);

    // An authoritative empty response is a current negative on its own merits,
    // and it reads as Observing rather than as a fabricated activity.
    let absence = insight_of("v-authoritative-absence");
    assert_eq!(absence["currentValidatorStatus"], "not_validator");
    assert_eq!(absence["currentValidatorStatusState"], "current");
    assert_eq!(absence["activity"], "observing");
    assert_eq!(absence["activityState"], "current");

    let not_configured = insight_of("v-not-configured");
    assert_eq!(not_configured["currentValidatorStatus"], "unknown");
    assert_eq!(not_configured["activity"], "unknown");
    assert_eq!(not_configured["activityState"], "unknown");

    // A locked identity is still a currently valid staking identity, yet it is
    // never presented as normal operation.
    let locked = insight_of("v-locked");
    assert_eq!(locked["currentValidatorStatus"], "validator");
    assert_eq!(locked["currentValidatorStatusState"], "current");
    assert_eq!(locked["currentValidatorStatusQualifier"], "locked");
    assert_eq!(locked["activity"], "locked");

    // Verification in progress stays Unknown even while its evidence is fresh.
    let verifying = insight_of("v-verifying");
    assert_eq!(verifying["currentValidatorStatus"], "unknown");
    assert_eq!(verifying["currentValidatorStatusState"], "unknown");
    assert_eq!(verifying["activity"], "verifying");
    assert_eq!(verifying["activityState"], "stale");
}

/// Every automatic Validator identity surface is Owner-only.
#[tokio::test]
async fn automatic_identity_surfaces_are_owner_only() {
    let harness = Harness::boot().await;
    harness.seed_node(NODE_IDENTIFIED, "Identified").await;
    harness
        .seed_chain_observation(NODE_IDENTIFIED, Some(NETWORK_CHAIN_ID), Some(&enode('a')))
        .await;
    validator::discover_automatic_links(harness.state.db())
        .await
        .unwrap();
    let validator_id: String =
        sqlx::query_scalar("SELECT validator_id FROM validators WHERE validator_node_id = ?")
            .bind(p2p_key('a'))
            .fetch_one(harness.pool())
            .await
            .unwrap();

    let uris = [
        "/api/admin/v1/validator-identities".to_owned(),
        "/api/admin/v1/validators".to_owned(),
        format!("/api/admin/v1/validators/{validator_id}"),
        format!("/api/admin/v1/nodes/{NODE_IDENTIFIED}"),
    ];
    for uri in &uris {
        let response = harness.send(anonymous_get(uri)).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "anonymous access to {uri}"
        );
    }

    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    for uri in &uris {
        let response = harness.send(admin_get(uri, &viewer)).await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "Viewer access to {uri}"
        );
    }

    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let response = harness
        .send(admin_get("/api/admin/v1/validator-identities", &owner))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// Purging a Node removes the Node-specific association evidence while the
/// shared Validator identity and its retained history survive, and the deleted
/// association reads back as unavailable rather than reconstructed.
#[tokio::test]
async fn node_purge_keeps_the_shared_validator_identity_and_its_history() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;

    harness.seed_node(NODE_IDENTIFIED, "Identified").await;
    harness
        .seed_chain_observation(NODE_IDENTIFIED, Some(NETWORK_CHAIN_ID), Some(&enode('a')))
        .await;
    validator::discover_automatic_links(harness.state.db())
        .await
        .unwrap();
    let validator_id: String =
        sqlx::query_scalar("SELECT validator_id FROM validators WHERE validator_node_id = ?")
            .bind(p2p_key('a'))
            .fetch_one(harness.pool())
            .await
            .unwrap();

    let now = harness.now("0 seconds").await;
    sqlx::query("INSERT INTO validator_ranking_history (history_id, validator_id, previous_rank, current_rank, observed_at, provider_timestamp, observation_key) VALUES ('history-1', ?, NULL, 7, ?, NULL, 'observation-1')")
        .bind(&validator_id)
        .bind(&now)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, counter_state, updated_at) VALUES (?, 'platscan', 'success', NULL, NULL, 'active', ?, ?, 'normal', ?)")
        .bind(&validator_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(harness.pool())
        .await
        .unwrap();

    let response = harness
        .send(admin_get("/api/admin/v1/validators", &owner))
        .await;
    let before = body_json(response).await;
    let entry = before
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["validatorId"] == validator_id.as_str())
        .unwrap();
    assert_eq!(entry["linkCount"], 1);

    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{NODE_IDENTIFIED}/purge"),
            &owner,
            &format!("{{\"confirmNodeId\":\"{NODE_IDENTIFIED}\"}}"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    let node_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE node_id = ?")
        .bind(NODE_IDENTIFIED)
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(node_rows, 0);
    let link_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM node_validator_links WHERE validator_id = ?")
            .bind(&validator_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(link_rows, 0);
    let identity_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM node_validator_identity_status WHERE node_id = ?")
            .bind(NODE_IDENTIFIED)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(identity_rows, 0);

    // The shared Validator survives with its retained Provider evidence.
    let response = harness
        .send(admin_get("/api/admin/v1/validators", &owner))
        .await;
    let after = body_json(response).await;
    let entry = after
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["validatorId"] == validator_id.as_str())
        .expect("the purged Node's Validator must survive the purge");
    assert_eq!(entry["linkCount"], 0);
    assert_eq!(entry["insight"]["outcome"], "success");
    assert_eq!(entry["insight"]["currentValidatorStatus"], "validator");

    // The deleted association reads back as unavailable, not reconstructed.
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/validators/{validator_id}"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let detail = body_json(response).await;
    assert_eq!(detail["links"].as_array().unwrap().len(), 0);

    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/validators/{validator_id}/history"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let history = body_json(response).await;
    assert_eq!(history["entries"].as_array().unwrap().len(), 1);

    let identities = identities(&harness, &owner).await;
    let listed = identities
        .iter()
        .filter(|identity| identity["nodeId"] == NODE_IDENTIFIED)
        .count();
    assert_eq!(
        listed, 0,
        "the purged Node must not appear in the identity coverage list"
    );
}
