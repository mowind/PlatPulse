//! Tests of the Validator schema migrations replayed over historical fixtures.

use tempfile::tempdir;

use crate::database::{ServerDatabaseConfig, initialize};

use crate::validator::insight::load_insight;
use crate::validator::refresh::refresh_all;
use crate::validator::test_helpers::FakeProvider;

/// A migrator restricted to the given schema version, so a test can build a
/// historical fixture and apply exactly one forward migration.
fn migrator_through(version: i64) -> sqlx::migrate::Migrator {
    use std::borrow::Cow;

    sqlx::migrate::Migrator {
        migrations: Cow::Owned(
            crate::database::SERVER_MIGRATOR
                .iter()
                .filter(|migration| migration.version <= version)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    }
}

#[cfg_attr(storage_postgres, ignore = "it replays a historical SQLite migration")]
#[tokio::test]
async fn candidate_activity_migration_preserves_existing_last_good_rows() {
    if !crate::test_support::skip_unless_sqlite("it replays a historical SQLite migration") {
        return;
    }
    let dir = tempdir().unwrap();
    let path = dir.path().join("server.db");
    let pool = crate::test_support::unmigrated_sqlite_pool(Some(&path)).await;
    // Build the schema immediately before 0060, then seed a last-good
    // Validator insight exactly as the pre-split Server would have stored it.
    migrator_through(59).run(&pool).await.unwrap();
    seed_legacy_network_agent_and_node(&pool).await;
    sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-keep', 'platon-mainnet', '0xkeep', NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, stake_amount, counter_state, change_state, candidate_observations, updated_at) VALUES ('validator-keep', 'platscan', 'success', '2026-01-01T00:00:00Z', 'active', '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z', '42', 'normal', 'normal', 0, '2026-01-02T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    // Reproduce the runtime failure when a Candidate observation reaches
    // a database that has not applied migration 0060 yet.
    let error = sqlx::query(
            "UPDATE current_validator_insights SET activity = 'candidate' WHERE validator_id = 'validator-keep'",
        )
        .execute(&pool)
        .await
        .unwrap_err();
    let database_error = error.as_database_error().unwrap();
    assert_eq!(database_error.code().as_deref(), Some("275"));
    assert_eq!(
        database_error.message(),
        "CHECK constraint failed: activity IN ('active', 'producing', 'exiting', 'exited', 'verifying', 'locked')"
    );
    pool.close().await;
    #[cfg(unix)]
    restrict_database_permissions(&path);

    let db = initialize(ServerDatabaseConfig::new(&path)).await.unwrap();

    // The table rebuild copies the stored row verbatim: no last-good value
    // is rewritten or erased by the widened CHECK.
    let row: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT outcome, activity, stake_amount FROM current_validator_insights WHERE validator_id = 'validator-keep'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        row,
        (
            "success".to_owned(),
            Some("active".to_owned()),
            Some("42".to_owned())
        )
    );
    // The widened CHECK now admits the split Candidate value.
    sqlx::query(
            "UPDATE current_validator_insights SET activity = 'candidate' WHERE validator_id = 'validator-keep'",
        )
        .execute(db.pool())
        .await
        .unwrap();
    let activity: Option<String> = sqlx::query_scalar(
        "SELECT activity FROM current_validator_insights WHERE validator_id = 'validator-keep'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(activity.as_deref(), Some("candidate"));
}

#[cfg_attr(storage_postgres, ignore = "it replays a historical SQLite migration")]
#[tokio::test]
async fn migration_0046_adds_the_delegation_percentage_without_backfilling_legacy_rows() {
    if !crate::test_support::skip_unless_sqlite("it replays a historical SQLite migration") {
        return;
    }
    // Build the schema exactly as it existed immediately before migration
    // 0046, seed one last-good row, then apply 0046 directly. The full
    // startup path also runs the destructive #174 cutover (0053), which
    // removes an unlinked legacy Validator, so scoping this test to 0046
    // keeps it about 0046.
    let dir = tempdir().unwrap();
    let path = dir.path().join("server.db");
    let pool = crate::test_support::unmigrated_sqlite_pool(Some(&path)).await;
    migrator_through(45).run(&pool).await.unwrap();
    sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('platon-mainnet', 'Mainnet', '0x0', 1, 1, 'lat', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-legacy', 'platon-mainnet', '0xabc', NULL, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, last_attempt_received_at, last_good_received_at, block_count, expected_block_count, gen_blocks_rate, updated_at) VALUES ('validator-legacy', 'platscan', 'success', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 100, 110, '75.5', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    migrator_through(46).run(&pool).await.unwrap();
    // Schema 46 predates several columns of the current INSIGHT_SELECT, so
    // this test reads only the columns that exist at that version.
    #[derive(sqlx::FromRow)]
    struct LegacyInsightRow {
        block_count: Option<i64>,
        expected_block_count: Option<i64>,
        gen_blocks_rate: Option<String>,
        delegation_reward_percentage: Option<String>,
    }
    let insight = sqlx::query_as::<_, LegacyInsightRow>(
            "SELECT block_count, expected_block_count, gen_blocks_rate, delegation_reward_percentage FROM current_validator_insights WHERE validator_id = 'validator-legacy'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    // The upgrade keeps every existing last-good value...
    assert_eq!(insight.block_count, Some(100));
    assert_eq!(insight.expected_block_count, Some(110));
    assert_eq!(insight.gen_blocks_rate.as_deref(), Some("75.5"));
    // ... and the new column stays Unknown instead of acquiring a value.
    assert_eq!(insight.delegation_reward_percentage, None);
    pool.close().await;
}

#[cfg_attr(storage_postgres, ignore = "it replays a historical SQLite migration")]
#[tokio::test]
async fn migration_0047_clears_the_untrusted_legacy_detail_rank() {
    if !crate::test_support::skip_unless_sqlite("it replays a historical SQLite migration") {
        return;
    }
    // Build the schema immediately before 0047, seed the old detail-alias
    // rank, then apply 0047 directly. The full startup path also runs the
    // destructive #174 cutover (0053), which removes an unlinked legacy
    // Validator, so scoping this test to 0047 keeps it about 0047.
    let dir = tempdir().unwrap();
    let path = dir.path().join("server.db");
    let pool = crate::test_support::unmigrated_sqlite_pool(Some(&path)).await;
    migrator_through(46).run(&pool).await.unwrap();
    sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('platon-mainnet', 'Mainnet', '0x0', 1, 1, 'lat', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-legacy-rank', 'platon-mainnet', '0xabc', NULL, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, last_attempt_received_at, last_good_received_at, rank, block_count, updated_at) VALUES ('validator-legacy-rank', 'platscan', 'success', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 5, 100, '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    migrator_through(47).run(&pool).await.unwrap();
    // Inspect the pinned historical schema, not the current row shape.
    let (rank, rank_outcome, block_count): (Option<i64>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT rank, rank_outcome, block_count FROM current_validator_insights WHERE validator_id = $1")
                .bind("validator-legacy-rank").fetch_one(&pool).await.unwrap();
    // The detail alias was never an authoritative ranking source, so the
    // upgrade drops it rather than exposing an untrusted position.
    assert_eq!(rank, None);
    assert_eq!(rank_outcome, None);
    assert_eq!(block_count, Some(100));
    pool.close().await;
}

/// A raw pool at the schema immediately before the one-time #174 Validator
/// model cutover (0053), for seeding a legacy generation.
async fn schema_before_validator_model_migration(path: &std::path::Path) -> sqlx::AnyPool {
    let pool = crate::test_support::unmigrated_sqlite_pool(Some(path)).await;
    migrator_through(52).run(&pool).await.unwrap();
    pool
}

/// Seed one Validator generation: a Link on node-1 whose `origin` is either
/// the legacy `manual` model or the automatic #173 model, plus a current
/// snapshot, ranking/counter history and daily/monthly aggregates.
async fn seed_validator_generation(
    pool: &sqlx::AnyPool,
    validator_id: &str,
    key: &str,
    origin: &str,
) {
    let now = "2026-01-01T00:00:00Z";
    sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ($1, 'platon-mainnet', $2, NULL, $3, $4)")
            .bind(validator_id)
            .bind(format!("0x{validator_id}"))
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ($1, 'node-1', $2, $3, $4, $5, NULL, $6, $7)")
            .bind(format!("link-{validator_id}"))
            .bind(validator_id)
            .bind(if origin == "manual" { Some("primary") } else { None })
            .bind(origin)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, stake_amount, reward_amount, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, last_observation_key, updated_at) VALUES ($1, 'platscan', 'success', $2, 'active', $3, $4, $5, '10', '5', 100, 'counter_reset', 'ranking_changed', 10, 20, 1, $6, $7, $8)")
            .bind(validator_id)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(key)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO validator_ranking_history (history_id, validator_id, previous_rank, current_rank, observed_at, observation_key) VALUES ($1, $2, 10, 20, $3, $4)")
            .bind(format!("rank-{validator_id}"))
            .bind(validator_id)
            .bind(now)
            .bind(key)
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO validator_counter_history (history_id, validator_id, counter_name, previous_value, current_value, observed_at, observation_key) VALUES ($1, $2, 'reward_amount', '5', '1', $3, $4)")
            .bind(format!("counter-{validator_id}"))
            .bind(validator_id)
            .bind(now)
            .bind(key)
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, source, observation_key) VALUES ($1, $2, 'UTC', '2026-01-01', '2026-01', $3, $4, 'platscan', $5)")
            .bind(format!("daily-{validator_id}"))
            .bind(validator_id)
            .bind(now)
            .bind(now)
            .bind(key)
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO validator_monthly_aggregates (aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, updated_at) VALUES ($1, $2, 'UTC', '2026-01', 1, $3, $4, $5)")
            .bind(format!("monthly-{validator_id}"))
            .bind(validator_id)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
}

async fn seed_legacy_network_agent_and_node(pool: &sqlx::AnyPool) {
    let now = "2026-01-01T00:00:00Z";
    sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('platon-mainnet', 'Mainnet', '0x0', 1, 1, 'lat', $1, $2)")
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-1', 1, $1, $2)")
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, lifecycle, visibility, inventory_revision, first_seen_at, updated_at, rpc_endpoint) VALUES ('node-1', 'agent-1', 'platon-mainnet', 'active', 'private', 1, $1, $2, 'http://127.0.0.1:1')")
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
}

#[cfg(unix)]
fn restrict_database_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// #174: the cutover deletes the legacy manual Links and the old Validator
/// snapshots/history/daily/monthly aggregates, but leaves Node monitoring
/// history, existing Incident evidence and Audit in place.
#[cfg_attr(storage_postgres, ignore = "it replays a historical SQLite migration")]
#[tokio::test]
async fn migration_0053_removes_the_legacy_generation_and_preserves_unrelated_evidence() {
    if !crate::test_support::skip_unless_sqlite("it replays a historical SQLite migration") {
        return;
    }
    let now = "2026-01-01T00:00:00Z";
    let dir = tempdir().unwrap();
    let path = dir.path().join("server.db");
    let pool = schema_before_validator_model_migration(&path).await;
    seed_legacy_network_agent_and_node(&pool).await;
    seed_validator_generation(&pool, "validator-manual", "obs-manual", "manual").await;
    // An automatic generation is the new model and must survive #174.
    seed_validator_generation(&pool, "validator-auto", "obs-auto", "automatic").await;
    // Node monitoring history must be preserved.
    sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES ('node-1', 10, 10, 20, 3, $1)")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, opened_at) VALUES ('node-1', 'peer-1', 'inbound', 0, 0, 0, $1)")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO node_metric_samples (node_id, metric, observed_at, received_at, value) VALUES ('node-1', 'process_cpu_percent', $1, $2, 1.5)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    // Existing Incident and Audit evidence must be retained. The rule row
    // only satisfies the Incident foreign key; the catalog is seeded later.
    sqlx::query("INSERT INTO alert_rules (rule_key, enabled, severity, version, condition_json, created_at, updated_at) VALUES ('validator.counter_reset', 1, 'critical', 1, '{}', $1, $2)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('incident-1', 'validator.counter_reset', 1, 'validator', 'validator-manual', 'critical', 'open', 1, $1, '{}')")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, created_at) VALUES (NULL, 'validator_created', 'validator', 'validator-manual', $1)")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    pool.close().await;
    #[cfg(unix)]
    restrict_database_permissions(&path);

    let db = initialize(ServerDatabaseConfig::new(&path)).await.unwrap();

    let origins: Vec<String> =
        sqlx::query_scalar("SELECT origin FROM node_validator_links ORDER BY origin")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(origins, vec!["automatic".to_owned()]);
    let validators: Vec<String> =
        sqlx::query_scalar("SELECT validator_id FROM validators ORDER BY validator_id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        validators,
        vec!["validator-auto".to_owned()],
        "only the legacy generation is removed"
    );
    for table in [
        "current_validator_insights",
        "validator_ranking_history",
        "validator_counter_history",
        "validator_daily_snapshots",
        "validator_monthly_aggregates",
    ] {
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT validator_id FROM {table} ORDER BY validator_id"
        ))
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec!["validator-auto".to_owned()],
            "{table} must keep the automatic generation and drop the legacy one"
        );
    }
    for table in [
        "block_history_state",
        "peer_presence_intervals",
        "node_metric_samples",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1, "{table} is Node history and must survive");
    }
    let incidents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_incidents")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(incidents, 1, "existing Incident evidence must survive");
    let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(audits, 1, "necessary Audit evidence must survive");
    let marker: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM validator_model_migration WHERE migration_key = 'validator_model'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(marker, 1, "the cutover marker must be recorded");
    assert_eq!(
        db.schema_version().await.unwrap(),
        crate::database::SERVER_SCHEMA_VERSION
    );
}

/// #174: removing a legacy generation with an open counter-reset Incident
/// must not be read as a recovery, and a later Provider pass must not
/// re-open the generation that was removed.
#[cfg_attr(storage_postgres, ignore = "it replays a historical SQLite migration")]
#[tokio::test]
async fn legacy_generation_removal_does_not_resolve_open_incidents_or_reaccumulate() {
    if !crate::test_support::skip_unless_sqlite("it replays a historical SQLite migration") {
        return;
    }
    let now = "2026-01-01T00:00:00Z";
    let dir = tempdir().unwrap();
    let path = dir.path().join("server.db");
    let pool = schema_before_validator_model_migration(&path).await;
    seed_legacy_network_agent_and_node(&pool).await;
    seed_validator_generation(&pool, "validator-legacy", "obs-1", "manual").await;
    // An open counter-reset Incident and its firing evaluation state belong
    // to the legacy generation and must survive as evidence.
    sqlx::query("INSERT INTO alert_rules (rule_key, enabled, severity, version, condition_json, created_at, updated_at) VALUES ('validator.counter_reset', 1, 'critical', 1, '{}', $1, $2)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO alert_rule_state (rule_key, subject_kind, subject_key, state, since, input_kind, last_evaluated_at) VALUES ('validator.counter_reset', 'validator', 'validator-legacy', 'firing', $1, 'known', $2)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('incident-legacy', 'validator.counter_reset', 1, 'validator', 'validator-legacy', 'critical', 'open', 1, $1, '{}')")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    pool.close().await;
    #[cfg(unix)]
    restrict_database_permissions(&path);

    let db = initialize(ServerDatabaseConfig::new(&path)).await.unwrap();

    let validators: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validators")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(validators, 0, "the legacy generation is removed");
    let snapshots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validator_daily_snapshots")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(snapshots, 0);
    // The Provider pass now sees no Validator to refresh, so it cannot
    // recreate the removed generation.
    let provider = FakeProvider::default();
    let summary = refresh_all(&db, &provider).await.unwrap();
    assert_eq!(summary.attempted, 0);
    assert!(
        load_insight(&db, "validator-legacy")
            .await
            .unwrap()
            .is_none()
    );
    // The reference boundary is closed too: an old insight cannot be
    // re-inserted for a removed identity.
    let reinsert = sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, last_attempt_received_at, updated_at) VALUES ('validator-legacy', 'platscan', 'success', $1, $2)")
            .bind(now)
            .bind(now)
            .execute(db.pool())
            .await;
    assert!(
        reinsert.is_err(),
        "a removed identity must not be re-openable"
    );
    // The cutover must not be read as a recovery: the Incident and its
    // firing evaluation state stay exactly as they were.
    let incident: (String, String) = sqlx::query_as(
        "SELECT state, subject_key FROM alert_incidents WHERE incident_id = 'incident-legacy'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(incident, ("open".to_owned(), "validator-legacy".to_owned()));
    let firing: String = sqlx::query_scalar(
            "SELECT state FROM alert_rule_state WHERE rule_key = 'validator.counter_reset' AND subject_key = 'validator-legacy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(firing, "firing");
}

/// #174: a failure after the destructive statements must roll the whole
/// migration back and stop startup, never leaving a half-migrated database.
#[cfg_attr(storage_postgres, ignore = "it replays a historical SQLite migration")]
#[tokio::test]
async fn a_failed_validator_model_migration_rolls_back_instead_of_serving_half_migrated_state() {
    if !crate::test_support::skip_unless_sqlite("it replays a historical SQLite migration") {
        return;
    }
    let dir = tempdir().unwrap();
    let path = dir.path().join("server.db");
    let pool = schema_before_validator_model_migration(&path).await;
    seed_legacy_network_agent_and_node(&pool).await;
    seed_validator_generation(&pool, "validator-legacy", "obs-1", "manual").await;
    // Sabotage the marker table so the migration's final INSERT fails only
    // after every deletion has run, exercising rollback rather than a
    // failure before the destructive statements.
    sqlx::query("CREATE TABLE validator_model_migration (migration_key TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    #[cfg(unix)]
    restrict_database_permissions(&path);

    let error = match initialize(ServerDatabaseConfig::new(&path)).await {
        Ok(_) => panic!("a failed cutover must not return a usable database"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        crate::database::ServerDatabaseError::Migration(_)
    ));

    let pool = crate::test_support::unmigrated_sqlite_pool(Some(&path)).await;
    let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 52, "a failed migration must not be recorded");
    let links: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM node_validator_links WHERE origin = 'manual'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        links, 1,
        "the manual generation must survive a failed cutover"
    );
    let snapshots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validator_daily_snapshots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(snapshots, 1);
    let marker: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validator_model_migration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(marker, 0);
}
