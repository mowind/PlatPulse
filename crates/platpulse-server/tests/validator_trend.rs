//! Owner-only Validator daily-snapshot trend acceptance through the real router
//! (issue #219, part of #202 stage 3).
//!
//! The trend answers the configured IANA calendar day and month boundaries
//! mapped back into the UTC investigation coordinate, discloses its own
//! coverage and gaps, keeps cumulative Provider counters labelled cumulative,
//! pages strictly older without a hole or a repeat, counts rows stored in
//! another timezone instead of merging them, and keeps the shared Validator
//! history readable after the Node it was linked to is purged. These tests build
//! the full application with build_app against a temporary SQLite database, run
//! the real discovery pass, and read the trend back over HTTP.

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
const NETWORK_CHAIN_ID: u64 = 210425;
const NETWORK_P2P_ID: u64 = 210425;
const NETWORK_HRP: &str = "lat";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer","password":"viewer password"}"#;
const AGENT_ID: &str = "0195f2a1-0019-4019-8019-000000000019";
const NODE_IDENTIFIED: &str = "0195f2a1-0019-4019-8019-000000000201";

/// A fresh Server (real temp SQLite + pepper) whose configured Validator
/// calendar is the IANA timezone this test asks about, the full router, and the
/// pieces needed to authenticate.
struct Harness {
    /// Kept alive for the lifetime of the harness so the temporary database
    /// directory is removed when the test finishes.
    _dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
    /// A Server configured to answer the given IANA timezone, exactly as the
    /// validator_provider.timezone setting configures it in production.
    async fn boot(timezone: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let harness = Self::open(dir, timezone).await;
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
            NETWORK_CHAIN_ID,
            NETWORK_P2P_ID,
            NETWORK_HRP,
        )
        .await
        .unwrap();
        harness.seed_agent().await;
        harness
    }

    async fn open(dir: TempDir, timezone: &str) -> Self {
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
        let state =
            AppState::new(database, None, auth).with_validator_timezone(timezone.to_owned());
        let app = http::build_app(state.clone());
        Self {
            _dir: dir,
            state,
            app,
        }
    }

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

    async fn seed_chain_observation(&self, node_id: &str, enode: &str) {
        let now = self.now("0 seconds").await;
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, enode, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(node_id)
            .bind(NETWORK_GENESIS)
            .bind(NETWORK_CHAIN_ID as i64)
            .bind(NETWORK_P2P_ID as i64)
            .bind(NETWORK_HRP)
            .bind(enode)
            .bind(&now)
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// A Node the real discovery pass identifies, plus the Validator row and the
    /// automatic Link interval that pass creates.
    async fn seed_identified_node(&self) -> String {
        self.seed_node(NODE_IDENTIFIED, "Identified").await;
        self.seed_chain_observation(NODE_IDENTIFIED, &enode('a'))
            .await;
        let summary = validator::discover_automatic_links(self.state.db())
            .await
            .unwrap();
        assert_eq!(summary.identified, 1);
        assert_eq!(summary.newly_linked, 1);
        sqlx::query_scalar("SELECT validator_id FROM validators WHERE validator_node_id = ?")
            .bind(p2p_key('a'))
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    /// One stored Validator day: the sample time is the Provider timestamp when
    /// the Provider gave one and the receipt time otherwise, exactly like the
    /// production writer.
    async fn seed_trend_day(
        &self,
        validator_id: &str,
        timezone: &str,
        local_date: &str,
        received_at: &str,
        provider_timestamp: Option<&str>,
    ) {
        sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'platsScan', ?, 12, '1000.000000', '25.000000', '0.05', 40, 5, 900)")
            .bind(format!("snapshot-{timezone}-{local_date}"))
            .bind(validator_id)
            .bind(timezone)
            .bind(local_date)
            .bind(&local_date[..7])
            .bind(provider_timestamp.unwrap_or(received_at))
            .bind(received_at)
            .bind(provider_timestamp)
            .bind(format!("observation-{timezone}-{local_date}"))
            .execute(self.pool())
            .await
            .unwrap();
    }

    fn pool(&self) -> &SqlitePool {
        self.state.db().pool()
    }

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

fn trend_uri(validator_id: &str, query: &str) -> String {
    format!("/api/admin/v1/validators/{validator_id}/trend{query}")
}

async fn trend(harness: &Harness, session: &Session, validator_id: &str, query: &str) -> Value {
    let response = harness
        .send(admin_get(&trend_uri(validator_id, query), session))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

/// The Owner reads the configured local calendar, its real UTC stretch, its own
/// coverage and gaps, and strictly older pages, while a Viewer and an anonymous
/// caller are refused before any of it is answered.
#[tokio::test]
async fn trend_answers_the_configured_calendar_for_the_owner_only() {
    // +05:45 is a real offset that is neither a whole hour nor UTC, so a
    // silently UTC-aligned answer cannot pass by accident.
    let harness = Harness::boot("Asia/Kathmandu").await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let validator_id = harness.seed_identified_node().await;

    harness
        .seed_trend_day(
            &validator_id,
            "Asia/Kathmandu",
            "2026-02-01",
            "2026-02-01T00:01:00Z",
            None,
        )
        .await;
    harness
        .seed_trend_day(
            &validator_id,
            "Asia/Kathmandu",
            "2026-02-03",
            "2026-02-03T00:00:30Z",
            Some("2026-02-03T00:00:00Z"),
        )
        .await;

    // An anonymous caller and a Viewer never reach the trend projection.
    let response = harness
        .send(anonymous_get(&trend_uri(&validator_id, "?limit=1")))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = harness
        .send(admin_get(&trend_uri(&validator_id, "?limit=1"), &viewer))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let page = trend(
        &harness,
        &owner,
        &validator_id,
        "?from=2026-02-01T00:00:00Z&to=2026-02-04T00:00:00Z",
    )
    .await;
    assert_eq!(page["validatorId"], validator_id.as_str());
    assert_eq!(page["networkKey"], NETWORK_KEY);
    assert_eq!(page["timezone"], "Asia/Kathmandu");
    // Cumulative Provider counters stay labelled cumulative: this answer is not
    // period earnings, net profit, or a difference between two samples.
    assert_eq!(page["counterSemantics"], "cumulative");
    assert_eq!(page["requestedFrom"], "2026-02-01T00:00:00Z");
    assert_eq!(page["requestedTo"], "2026-02-04T00:00:00Z");
    assert_eq!(page["requestedFromLocalDate"], "2026-02-01");
    assert_eq!(page["requestedToLocalDate"], "2026-02-04");
    assert_eq!(page["requestedDays"], 4);
    assert_eq!(page["clamped"], false);
    assert_eq!(page["expectedDays"], 4);
    assert_eq!(page["observedDays"], 2);
    assert_eq!(page["missingDays"], 2);
    assert_eq!(page["coverage"], "partial");
    assert_eq!(page["answeredFromLocalDate"], "2026-02-01");
    assert_eq!(page["answeredToLocalDate"], "2026-02-04");
    assert_eq!(page["firstObservedLocalDate"], "2026-02-01");
    assert_eq!(page["lastObservedLocalDate"], "2026-02-03");
    assert_eq!(page["truncated"], false);
    assert_eq!(page["continuation"], Value::Null);
    assert_eq!(page["foreignRows"], 0);
    assert!(page["foreignTimezones"].as_array().unwrap().is_empty());
    assert_eq!(page["associationsTruncated"], false);
    assert_eq!(page["deletedNodes"], 0);
    assert_eq!(page["associationHistoryPartial"], false);

    // The local day and its month boundary are the configured calendar's, mapped
    // back into UTC instead of a UTC-aligned bucket.
    let points = page["points"].as_array().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0]["localDate"], "2026-02-01");
    assert_eq!(points[0]["monthKey"], "2026-02");
    assert_eq!(points[0]["dayStart"], "2026-01-31T18:15:00Z");
    assert_eq!(points[0]["dayEnd"], "2026-02-01T18:15:00Z");
    assert_eq!(points[0]["rank"], 12);
    assert_eq!(points[0]["stakeAmount"], "1000.000000");
    assert_eq!(points[0]["delegatorCount"], 40);
    // No Provider timestamp means the sample time is the receipt and the delay is
    // unknown, which is not a fresh zero delay.
    assert_eq!(points[0]["sampleTime"], "receipt");
    assert_eq!(points[0]["delaySeconds"], Value::Null);
    assert_eq!(points[0]["clockSuspect"], false);
    assert_eq!(points[1]["localDate"], "2026-02-03");
    assert_eq!(points[1]["sampleTime"], "provider");
    assert_eq!(points[1]["delaySeconds"], 30);
    let months = page["months"].as_array().unwrap();
    assert_eq!(months.len(), 1);
    assert_eq!(months[0]["monthKey"], "2026-02");
    assert_eq!(months[0]["monthStart"], "2026-01-31T18:15:00Z");
    assert_eq!(months[0]["monthEnd"], "2026-02-28T18:15:00Z");
    assert_eq!(months[0]["observedDays"], 2);
    // A day inside the answered stretch with no snapshot is a proven gap.
    let gaps = page["gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 2, "unread answered days are gaps: {page}");
    assert_eq!(gaps[0]["fromLocalDate"], "2026-02-02");
    assert_eq!(gaps[0]["days"], 1);

    // The shared Validator's association is read here too, so the trend and the
    // Link panels cannot disagree about the same interval.
    let associations = page["associations"].as_array().unwrap();
    assert_eq!(associations.len(), 1);
    assert_eq!(associations[0]["nodeId"], NODE_IDENTIFIED);
    assert_eq!(associations[0]["origin"], "automatic");
    assert_eq!(associations[0]["nodeLifecycle"], "active");
    assert_eq!(associations[0]["validUntil"], Value::Null);
    assert_eq!(associations[0]["current"], true);

    // Pagination is disclosed and the older page continues strictly below the
    // newest answered day, without a hole or a repeat.
    let first = trend(
        &harness,
        &owner,
        &validator_id,
        "?from=2026-02-01T00:00:00Z&to=2026-02-04T00:00:00Z&limit=1",
    )
    .await;
    assert_eq!(first["truncated"], true);
    assert_eq!(first["continuation"], "2026-02-03");
    assert_eq!(first["points"].as_array().unwrap().len(), 1);
    assert_eq!(first["points"][0]["localDate"], "2026-02-03");
    assert_eq!(first["coverage"], "partial");

    let older = trend(
        &harness,
        &owner,
        &validator_id,
        "?from=2026-02-01T00:00:00Z&to=2026-02-04T00:00:00Z&limit=1&before=2026-02-03",
    )
    .await;
    assert_eq!(older["truncated"], false);
    assert_eq!(older["continuation"], Value::Null);
    assert_eq!(older["points"].as_array().unwrap().len(), 1);
    assert_eq!(older["points"][0]["localDate"], "2026-02-01");
    assert_eq!(older["answeredFromLocalDate"], "2026-02-01");
    assert_eq!(older["answeredToLocalDate"], "2026-02-02");
    assert_eq!(older["expectedDays"], 2);
    assert_eq!(older["observedDays"], 1);
    // The cursor really narrowed the claim: 2026-02-03 and 2026-02-04 are no
    // longer part of this page at all, and the one day this page does claim and
    // did not observe is the only gap it may report.
    let older_gaps = older["gaps"].as_array().unwrap();
    assert_eq!(
        older_gaps.len(),
        1,
        "only the claimed day is a gap: {older}"
    );
    assert_eq!(older_gaps[0]["fromLocalDate"], "2026-02-02");
    // Both pages together are the two observed days, with no repeat.
    let delivered = [
        first["points"][0]["localDate"].as_str().unwrap(),
        older["points"][0]["localDate"].as_str().unwrap(),
    ];
    assert_eq!(delivered, ["2026-02-03", "2026-02-01"]);

    // An unknown Validator is a plain 404 and a Sanitized error never leaks the
    // database behind it.
    let response = harness
        .send(admin_get(
            &trend_uri("validator-missing", "?limit=1"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let error = body_json(response).await;
    assert_eq!(error["error"]["code"], "not_found");

    let response = harness
        .send(admin_get(
            &trend_uri(
                &validator_id,
                "?from=2026-06-02T00:00:00Z&to=2026-06-01T00:00:00Z",
            ),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = body_json(response).await;
    assert_eq!(error["error"]["code"], "invalid_request");
    assert_eq!(
        error["error"]["message"],
        "invalid Validator trend window: from must not be later than to"
    );
}

/// A row stored under another timezone is counted and named, never silently
/// merged into the configured calendar or re-bucketed to UTC.
#[tokio::test]
async fn a_row_stored_in_another_timezone_is_disclosed_instead_of_merged() {
    let harness = Harness::boot("Asia/Tokyo").await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let validator_id = harness.seed_identified_node().await;

    harness
        .seed_trend_day(
            &validator_id,
            "Asia/Tokyo",
            "2026-02-01",
            "2026-01-31T15:00:30Z",
            Some("2026-01-31T15:00:00Z"),
        )
        .await;
    harness
        .seed_trend_day(
            &validator_id,
            "UTC",
            "2026-02-01",
            "2026-02-01T00:00:30Z",
            Some("2026-02-01T00:00:00Z"),
        )
        .await;

    // The whole configured local day 2026-02-01 in Tokyo is 2026-01-31T15:00Z to
    // 2026-02-01T15:00Z; the UTC row shares the local date but not the bucket.
    let page = trend(
        &harness,
        &owner,
        &validator_id,
        "?from=2026-01-31T15:00:00Z&to=2026-02-01T03:00:00Z",
    )
    .await;
    assert_eq!(page["expectedDays"], 1);
    assert_eq!(page["observedDays"], 1);
    assert_eq!(page["missingDays"], 0);
    assert_eq!(page["coverage"], "complete");
    assert_eq!(page["points"].as_array().unwrap().len(), 1);
    assert_eq!(page["points"][0]["localDate"], "2026-02-01");
    assert_eq!(page["points"][0]["dayStart"], "2026-01-31T15:00:00Z");
    assert_eq!(page["foreignRows"], 1);
    assert_eq!(
        page["foreignTimezones"].as_array().unwrap(),
        &vec![Value::from("UTC")]
    );
}

/// Story 68: purging the Node removes its association rows, but the shared
/// Validator's retained snapshot days keep answering and the missing
/// association is disclosed as unavailable rather than never having existed.
#[tokio::test]
async fn a_purged_node_leaves_the_retained_days_and_a_partial_association_notice() {
    let harness = Harness::boot("Asia/Tokyo").await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let validator_id = harness.seed_identified_node().await;

    harness
        .seed_trend_day(
            &validator_id,
            "Asia/Tokyo",
            "2026-02-01",
            "2026-01-31T15:00:30Z",
            Some("2026-01-31T15:00:00Z"),
        )
        .await;
    harness
        .seed_trend_day(
            &validator_id,
            "Asia/Tokyo",
            "2026-02-02",
            "2026-02-01T15:00:30Z",
            Some("2026-02-01T15:00:00Z"),
        )
        .await;

    let window = "?from=2026-01-31T15:00:00Z&to=2026-02-02T03:00:00Z";
    let before = trend(&harness, &owner, &validator_id, window).await;
    assert_eq!(before["observedDays"], 2);
    assert_eq!(before["coverage"], "complete");
    assert_eq!(before["associations"].as_array().unwrap().len(), 1);
    assert_eq!(before["deletedNodes"], 0);
    assert_eq!(before["associationHistoryPartial"], false);

    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{NODE_IDENTIFIED}/purge"),
            &owner,
            &format!("{{\"confirmNodeId\":\"{NODE_IDENTIFIED}\"}}"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    let after = trend(&harness, &owner, &validator_id, window).await;
    // The independent Validator history is never deleted by Purge.
    assert_eq!(after["validatorId"], validator_id.as_str());
    assert_eq!(after["observedDays"], 2);
    assert_eq!(after["coverage"], "complete");
    assert_eq!(after["points"].as_array().unwrap().len(), 2);
    assert_eq!(after["gaps"].as_array().unwrap().len(), 0);
    // The deleted Node's association is unavailable, not reconstructed and not
    // reported as an interval that never existed.
    assert!(after["associations"].as_array().unwrap().is_empty());
    assert_eq!(after["associationsTruncated"], false);
    assert_eq!(after["deletedNodes"], 1);
    assert_eq!(after["associationHistoryPartial"], true);
    assert_eq!(after["counterSemantics"], "cumulative");

    // The retained rows are still the real stored days, not a fresh zero.
    assert_eq!(after["points"][1]["localDate"], "2026-02-02");
    assert_eq!(after["points"][1]["stakeAmount"], "1000.000000");
    assert_eq!(after["points"][1]["delaySeconds"], 30);
}
