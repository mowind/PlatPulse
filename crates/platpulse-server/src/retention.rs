//! Retention policies and bounded, safety-protected execution (issue #50,
//! design §11.3, webui.md §8.4).
//!
//! Phase 1 provided a fixed seven-day raw Block Summary cleanup. Phase 2
//! adds Owner-configurable per-family policies with fixed safety bounds,
//! read-only impact previews, and a batched Operation that never lowers the
//! historical high-water mark, never deletes coverage/gap/divergence state
//! or cumulative counters, never touches immutable Incident history, and
//! never removes Audit Events still referenced by Operations.

use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::http::AppState;

/// Phase 1 raw Block Summary retention baseline from design §11.3.
pub const RAW_BLOCK_SUMMARY_RETENTION_DAYS: i64 = 7;
/// Maximum number of raw rows removed by one cleanup invocation.
pub const RAW_BLOCK_SUMMARY_CLEANUP_BATCH: i64 = 128;
/// Phase 3 provides five-minute and hourly Peer aggregate history; the
/// legacy block-history aggregate families remain unsupported.
pub const RAW_BLOCK_HISTORY_AGGREGATES_SUPPORTED: bool = false;
/// Maximum number of rows removed by one bounded retention batch.
pub const RETENTION_BATCH: i64 = 128;

pub const FAMILY_RAW_BLOCK_SUMMARY: &str = "raw_block_summary";
pub const FAMILY_ONE_MINUTE_AGGREGATE: &str = "one_minute_aggregate";
pub const FAMILY_ONE_HOUR_AGGREGATE: &str = "one_hour_aggregate";
pub const FAMILY_HISTORY_GAP: &str = "history_gap";
pub const FAMILY_DIVERGENCE_OBSERVATION: &str = "divergence_observation";
pub const FAMILY_AUDIT_EVENT: &str = "audit_event";
pub const FAMILY_ALERT_NOTIFICATION: &str = "alert_notification";
pub const FAMILY_PEER_PRESENCE_INTERVAL: &str = "peer_presence_interval";
pub const FAMILY_PEER_AGGREGATE_5M: &str = "peer_aggregate_5m";
pub const FAMILY_PEER_AGGREGATE_1H: &str = "peer_aggregate_1h";
pub const FAMILY_VALIDATOR_DAILY_SNAPSHOT: &str = "validator_daily_snapshot";
pub const FAMILY_VALIDATOR_MONTHLY_AGGREGATE: &str = "validator_monthly_aggregate";

/// Policy defaults and safety bounds (design §11.3). `max_days = 0` means
/// no upper bound (long-term family); `retention_days = 0` keeps forever.
pub struct PolicyDefaults {
    pub family: &'static str,
    pub label: &'static str,
    pub default_days: i64,
    pub min_days: i64,
    pub max_days: i64,
    pub supported: bool,
}

pub const POLICY_CATALOG: [PolicyDefaults; 12] = [
    PolicyDefaults {
        family: FAMILY_RAW_BLOCK_SUMMARY,
        label: "Raw Block Summaries",
        default_days: 7,
        min_days: 1,
        max_days: 30,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_ONE_MINUTE_AGGREGATE,
        label: "1-Minute Aggregates",
        default_days: 90,
        min_days: 7,
        max_days: 365,
        supported: false,
    },
    PolicyDefaults {
        family: FAMILY_ONE_HOUR_AGGREGATE,
        label: "1-Hour Aggregates",
        default_days: 0,
        min_days: 0,
        max_days: 0,
        supported: false,
    },
    PolicyDefaults {
        family: FAMILY_HISTORY_GAP,
        label: "History Gap Records",
        default_days: 180,
        min_days: 180,
        max_days: 0,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_DIVERGENCE_OBSERVATION,
        label: "Divergence Evidence",
        default_days: 180,
        min_days: 180,
        max_days: 0,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_AUDIT_EVENT,
        label: "Audit Events",
        default_days: 365,
        min_days: 365,
        max_days: 0,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_ALERT_NOTIFICATION,
        label: "Alert Notification Events",
        default_days: 180,
        min_days: 90,
        max_days: 0,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_PEER_PRESENCE_INTERVAL,
        label: "Peer Presence Intervals",
        default_days: 30,
        min_days: 1,
        max_days: 365,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_PEER_AGGREGATE_5M,
        label: "Peer 5-Minute Aggregates",
        default_days: 90,
        min_days: 7,
        max_days: 365,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_PEER_AGGREGATE_1H,
        label: "Peer 1-Hour Aggregates",
        default_days: 0,
        min_days: 0,
        max_days: 0,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_VALIDATOR_DAILY_SNAPSHOT,
        label: "Validator Daily Snapshots",
        // Daily snapshots are the durable source for calendar-month rebuilds.
        // They are kept forever so delayed retries and restarts can never
        // re-insert an old day into a partially retained month.
        default_days: 0,
        min_days: 0,
        max_days: 0,
        supported: true,
    },
    PolicyDefaults {
        family: FAMILY_VALIDATOR_MONTHLY_AGGREGATE,
        label: "Validator Monthly Aggregates",
        // Monthly aggregates are derived, long-term reporting state and must
        // never be removed by a retention run.
        default_days: 0,
        min_days: 0,
        max_days: 0,
        supported: true,
    },
];

pub fn catalog_family(family: &str) -> Option<&'static PolicyDefaults> {
    POLICY_CATALOG.iter().find(|entry| entry.family == family)
}

#[derive(Debug, Clone)]
pub struct PolicyRow {
    pub family: String,
    pub retention_days: i64,
    pub min_days: i64,
    pub max_days: i64,
    pub supported: bool,
    pub enabled: bool,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

/// RFC3339 cutoff before which rows of a family may be removed.
pub fn family_cutoff(now: time::OffsetDateTime, retention_days: i64) -> time::OffsetDateTime {
    now - time::Duration::days(retention_days)
}

/// RFC3339 cutoff before which raw summaries may be removed.
pub fn raw_block_summary_cutoff(now: time::OffsetDateTime) -> time::OffsetDateTime {
    family_cutoff(now, RAW_BLOCK_SUMMARY_RETENTION_DAYS)
}

/// Delete at most one bounded batch of expired raw summaries.
///
/// The query is intentionally one short SQLite statement: repeated startup or
/// ingestion calls are idempotent, and a large historical table cannot make a
/// single cleanup transaction unbounded. The tables that preserve dedup and
/// recovery state are not joined or deleted here.
pub async fn cleanup_raw_block_summaries(
    pool: &SqlitePool,
    now: time::OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let cutoff = crate::auth::format_rfc3339(raw_block_summary_cutoff(now));
    let result = sqlx::query(
        "DELETE FROM block_summaries WHERE rowid IN (SELECT rowid FROM block_summaries WHERE accepted_at < ? ORDER BY accepted_at, node_id, block_number LIMIT ?)",
    )
    .bind(cutoff)
    .bind(RAW_BLOCK_SUMMARY_CLEANUP_BATCH)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Idempotent policy seeding with the design §11.3 defaults. Safe to call
/// at startup and from read handlers; existing rows are never rewritten.
pub async fn ensure_seeded(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let mut tx = pool.begin().await?;
    for policy in POLICY_CATALOG {
        sqlx::query(
            "INSERT OR IGNORE INTO retention_policies (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by) VALUES (?, ?, ?, ?, ?, 1, ?, 'defaults')",
        )
        .bind(policy.family)
        .bind(policy.default_days)
        .bind(policy.min_days)
        .bind(policy.max_days)
        .bind(policy.supported as i64)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

pub async fn list_policies(pool: &SqlitePool) -> Result<Vec<PolicyRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, i64, i64, i64, i64, i64, String, Option<String>)>(
        "SELECT family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by FROM retention_policies ORDER BY family",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                family,
                retention_days,
                min_days,
                max_days,
                supported,
                enabled,
                updated_at,
                updated_by,
            )| {
                PolicyRow {
                    family,
                    retention_days,
                    min_days,
                    max_days,
                    supported: supported == 1,
                    enabled: enabled == 1,
                    updated_at,
                    updated_by,
                }
            },
        )
        .collect())
}

/// Validate a proposed retention days value against the family's fixed
/// safety bounds. `0` means "keep forever" and is allowed only for
/// long-term families (max_days = 0 and min_days = 0).
pub fn validate_policy_days(family: &str, days: i64) -> Result<(), String> {
    let Some(catalog) = catalog_family(family) else {
        return Err(format!("unknown retention family {family}"));
    };
    if days < 0 {
        return Err("retention days must be zero or positive".to_owned());
    }
    if catalog.max_days == 0 {
        // Long-term family: either keep forever or stay above the floor.
        if catalog.min_days == 0 {
            if days != 0 {
                return Err(format!(
                    "{} is a long-term family and can only be kept forever (0 days)",
                    catalog.label
                ));
            }
        } else if days != 0 && days < catalog.min_days {
            return Err(format!(
                "{} cannot be lowered below {} days (design §11.3 safety floor)",
                catalog.label, catalog.min_days
            ));
        }
    } else if !(catalog.min_days..=catalog.max_days).contains(&days) {
        return Err(format!(
            "{} must be between {} and {} days (design §11.3 safety bounds)",
            catalog.label, catalog.min_days, catalog.max_days
        ));
    }
    Ok(())
}

/// Apply a validated policy change. The caller writes the Audit row in the
/// same transaction (mutation handlers audit every policy change).
pub async fn update_policy(
    pool: &SqlitePool,
    family: &str,
    retention_days: i64,
    actor_user_id: &str,
) -> Result<PolicyRow, String> {
    validate_policy_days(family, retention_days)?;
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    sqlx::query(
        "UPDATE retention_policies SET retention_days = ?, enabled = 1, updated_at = ?, updated_by = ? WHERE family = ?",
    )
    .bind(retention_days)
    .bind(&now)
    .bind(actor_user_id)
    .bind(family)
    .execute(pool)
    .await
    .map_err(|error| format!("retention policy update failed: {error}"))?;
    list_policies(pool)
        .await
        .map_err(|error| format!("retention policy reload failed: {error}"))?
        .into_iter()
        .find(|policy| policy.family == family)
        .ok_or_else(|| format!("unknown retention family {family}"))
}

/// Read-only impact estimate for a proposed policy value. Never writes;
/// used by the edit form before typed confirmation (webui.md §8.4).
pub async fn estimate_impact(
    pool: &SqlitePool,
    family: &str,
    retention_days: i64,
    now: time::OffsetDateTime,
) -> Result<(i64, bool), sqlx::Error> {
    let Some(catalog) = catalog_family(family) else {
        return Ok((0, true));
    };
    if !catalog.supported {
        return Ok((0, true));
    }
    if retention_days == 0 {
        return Ok((0, false));
    }
    let cutoff = crate::auth::format_rfc3339(family_cutoff(now, retention_days));
    let count = match family {
        FAMILY_RAW_BLOCK_SUMMARY => {
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE accepted_at < ?")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?
        }
        FAMILY_HISTORY_GAP => {
            sqlx::query_scalar("SELECT COUNT(*) FROM block_history_gaps WHERE resolved_at IS NOT NULL AND kind != 'permanent_gap' AND resolved_at < ?")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?
        }
        FAMILY_DIVERGENCE_OBSERVATION => {
            let divergences: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chain_divergence_observations WHERE retained_observed_at < ?")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?;
            let identity: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM block_identity_window WHERE retained_until < ?")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?;
            divergences + identity
        }
        FAMILY_AUDIT_EVENT => {
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE created_at < ? AND audit_event_id NOT IN (SELECT audit_event_id FROM operations WHERE audit_event_id IS NOT NULL)")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?
        }
        FAMILY_ALERT_NOTIFICATION => {
            sqlx::query_scalar("SELECT COUNT(*) FROM notification_events WHERE created_at < ?")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?
        }
        FAMILY_PEER_PRESENCE_INTERVAL => {
            sqlx::query_scalar("SELECT COUNT(*) FROM peer_presence_intervals WHERE closed_at IS NOT NULL AND closed_at < ?")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?
        }
        FAMILY_PEER_AGGREGATE_5M => {
            sqlx::query_scalar("SELECT COUNT(*) FROM peer_aggregate_5m WHERE bucket_start < ?")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?
        }
        FAMILY_PEER_AGGREGATE_1H => {
            sqlx::query_scalar("SELECT COUNT(*) FROM peer_aggregate_1h WHERE bucket_start < ?")
                .bind(&cutoff)
                .fetch_one(pool)
                .await?
        }
        _ => 0,
    };
    Ok((count, false))
}

/// Human-readable list of state that retention can never delete. Shown on
/// every retention surface so the safety contract stays explicit.
pub fn protected_state_notes() -> Vec<&'static str> {
    vec![
        "historical high-water marks",
        "coverage intervals",
        "open or permanent gap records",
        "cumulative block/transaction counters",
        "immutable Incident history",
        "Audit Events referenced by Operations",
        "Rule versions and policy rows",
        "open Peer presence intervals",
        "Validator daily snapshots and monthly aggregates",
    ]
}

// ---------------------------------------------------------------------------
// Retention run Operation (kind `retention_run`)
// ---------------------------------------------------------------------------

/// Internal execution plan stored inside the Operation's params. Each entry
/// is one physical table batch target; divergence evidence spans two tables.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct PlanEntry {
    pub family: String,
    pub table: String,
    /// RFC3339 cutoff frozen at plan time. Re-reading the live policy on
    /// every batch could lengthen a policy mid-run and leave `deleted <
    /// total` forever; the plan always executes against the snapshot the
    /// operator confirmed.
    pub cutoff: String,
    pub total: i64,
    pub deleted: i64,
}

/// Advance one retention run by one bounded batch. The run persists its
/// plan and progress in `params_json`/`progress_percent` and only reaches a
/// terminal state through `finalize` — so a crash never fabricates success.
pub async fn execute_step(
    state: &AppState,
    operation_id: &str,
) -> Result<(), crate::operations::OperationError> {
    let pool = state.db().pool();
    let mut params = crate::operations::operation_params(pool, operation_id).await?;
    let now = crate::auth::now_utc();

    let mut plan: Vec<PlanEntry> = match params.get("plan").and_then(Value::as_array) {
        Some(entries) => entries
            .iter()
            .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
            .collect(),
        None => build_plan(state, operation_id, &params, now).await?,
    };

    if plan.is_empty() {
        finish_run(state, operation_id, params, Vec::new()).await?;
        return Ok(());
    }

    // First entry with remaining work.
    let Some(index) = plan.iter().position(|entry| entry.deleted < entry.total) else {
        finish_run(state, operation_id, params, plan).await?;
        return Ok(());
    };

    if crate::operations::is_cancel_requested(state, operation_id).await? {
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_CANCELLED,
            None,
            &["retention"],
        )
        .await?;
        return Ok(());
    }

    let entry = &plan[index];
    let sql = batch_sql(&entry.table);
    let result = sqlx::query(sql).bind(&entry.cutoff).execute(pool).await;
    match result {
        Ok(result) => {
            let rows = result.rows_affected();
            plan[index].deleted += rows as i64;
        }
        Err(error) => {
            let _ = crate::operations::add_error(
                state,
                operation_id,
                "retention_batch_failed",
                &crate::redaction::redact_sensitive(&error.to_string()),
            )
            .await;
            let _ = crate::operations::finalize(
                state,
                operation_id,
                crate::operations::STATUS_FAILED,
                None,
                &["retention"],
            )
            .await;
            return Ok(());
        }
    }

    params["plan"] = serde_json::to_value(&plan)?;
    sqlx::query("UPDATE operations SET params_json = ? WHERE operation_id = ?")
        .bind(serde_json::to_string(&params)?)
        .bind(operation_id)
        .execute(pool)
        .await?;

    let total: i64 = plan.iter().map(|entry| entry.total).sum();
    let deleted: i64 = plan.iter().map(|entry| entry.deleted).sum();
    let percent = if total == 0 {
        100
    } else {
        (deleted * 100) / total
    };
    let entry = &plan[index];
    crate::operations::set_progress(
        state,
        operation_id,
        percent,
        &format!("{} {}/{}", entry.family, entry.deleted, entry.total),
    )
    .await?;
    Ok(())
}

/// Fixed SQL per physical table (never dynamic). Every statement binds the
/// plan entry's frozen RFC3339 cutoff as its single parameter.
fn batch_sql(table: &str) -> &'static str {
    match table {
        "block_summaries" => {
            "DELETE FROM block_summaries WHERE rowid IN (SELECT rowid FROM block_summaries WHERE accepted_at < ? ORDER BY accepted_at, node_id, block_number LIMIT 128)"
        }
        "block_history_gaps" => {
            "DELETE FROM block_history_gaps WHERE gap_id IN (SELECT gap_id FROM block_history_gaps WHERE resolved_at IS NOT NULL AND kind != 'permanent_gap' AND resolved_at < ? ORDER BY resolved_at LIMIT 128)"
        }
        "chain_divergence_observations" => {
            "DELETE FROM chain_divergence_observations WHERE rowid IN (SELECT rowid FROM chain_divergence_observations WHERE retained_observed_at < ? ORDER BY retained_observed_at LIMIT 128)"
        }
        "block_identity_window" => {
            "DELETE FROM block_identity_window WHERE rowid IN (SELECT rowid FROM block_identity_window WHERE retained_until < ? ORDER BY retained_until LIMIT 128)"
        }
        "audit_events" => {
            "DELETE FROM audit_events WHERE audit_event_id IN (SELECT audit_event_id FROM audit_events WHERE created_at < ? AND audit_event_id NOT IN (SELECT audit_event_id FROM operations WHERE audit_event_id IS NOT NULL) ORDER BY audit_event_id LIMIT 128)"
        }
        "notification_events" => {
            "DELETE FROM notification_events WHERE event_id IN (SELECT event_id FROM notification_events WHERE created_at < ? ORDER BY created_at LIMIT 128)"
        }
        "peer_presence_intervals" => {
            "DELETE FROM peer_presence_intervals WHERE interval_id IN (SELECT interval_id FROM peer_presence_intervals WHERE closed_at IS NOT NULL AND closed_at < ? ORDER BY closed_at, interval_id LIMIT 128)"
        }
        "peer_aggregate_5m" => {
            "DELETE FROM peer_aggregate_5m WHERE aggregate_id IN (SELECT aggregate_id FROM peer_aggregate_5m WHERE bucket_start < ? ORDER BY bucket_start, aggregate_id LIMIT 128)"
        }
        "peer_aggregate_1h" => {
            "DELETE FROM peer_aggregate_1h WHERE aggregate_id IN (SELECT aggregate_id FROM peer_aggregate_1h WHERE bucket_start < ? ORDER BY bucket_start, aggregate_id LIMIT 128)"
        }
        _ => "SELECT 1",
    }
}

/// Build the bounded execution plan from the requested families. `families:
/// null` means every enabled and supported policy. Unsupported/disabled
/// families are excluded; explicitly requested ones raise warnings.
async fn build_plan(
    state: &AppState,
    operation_id: &str,
    params: &Value,
    now: time::OffsetDateTime,
) -> Result<Vec<PlanEntry>, crate::operations::OperationError> {
    let pool = state.db().pool();
    let requested: Option<Vec<String>> =
        params
            .get("families")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
    let policies = list_policies(pool).await?;
    let mut plan: Vec<PlanEntry> = Vec::new();
    let mut warnings: Vec<(String, String)> = Vec::new();

    let scope: Vec<PolicyRow> = match &requested {
        Some(families) => {
            let mut rows: Vec<PolicyRow> = Vec::new();
            for family in families {
                let Some(policy) = policies.iter().find(|policy| &policy.family == family) else {
                    warnings.push((
                        "retention_unknown_family".to_owned(),
                        format!("{family}: unknown retention family, skipped"),
                    ));
                    continue;
                };
                if !policy.supported {
                    warnings.push((
                        "retention_unsupported".to_owned(),
                        format!(
                            "{}: aggregates are not produced in this phase, skipped",
                            policy.family
                        ),
                    ));
                    continue;
                }
                if !policy.enabled {
                    warnings.push((
                        "retention_disabled".to_owned(),
                        format!("{}: policy is disabled, skipped", policy.family),
                    ));
                    continue;
                }
                rows.push(policy.clone());
            }
            rows
        }
        None => policies
            .into_iter()
            .filter(|policy| policy.supported && policy.enabled)
            .collect(),
    };

    for policy in scope {
        if policy.retention_days == 0 {
            continue;
        }
        let cutoff = crate::auth::format_rfc3339(family_cutoff(now, policy.retention_days));
        let entries = match policy.family.as_str() {
            FAMILY_RAW_BLOCK_SUMMARY => vec![(
                "block_summaries".to_owned(),
                sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE accepted_at < ?")
                    .bind(&cutoff)
                    .fetch_one(pool)
                    .await?,
            )],
            FAMILY_HISTORY_GAP => vec![(
                "block_history_gaps".to_owned(),
                sqlx::query_scalar("SELECT COUNT(*) FROM block_history_gaps WHERE resolved_at IS NOT NULL AND kind != 'permanent_gap' AND resolved_at < ?")
                    .bind(&cutoff)
                    .fetch_one(pool)
                    .await?,
            )],
            FAMILY_DIVERGENCE_OBSERVATION => vec![
                (
                    "chain_divergence_observations".to_owned(),
                    sqlx::query_scalar("SELECT COUNT(*) FROM chain_divergence_observations WHERE retained_observed_at < ?")
                        .bind(&cutoff)
                        .fetch_one(pool)
                        .await?,
                ),
                (
                    "block_identity_window".to_owned(),
                    sqlx::query_scalar("SELECT COUNT(*) FROM block_identity_window WHERE retained_until < ?")
                        .bind(&cutoff)
                        .fetch_one(pool)
                        .await?,
                ),
            ],
            FAMILY_AUDIT_EVENT => vec![(
                "audit_events".to_owned(),
                sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE created_at < ? AND audit_event_id NOT IN (SELECT audit_event_id FROM operations WHERE audit_event_id IS NOT NULL)")
                    .bind(&cutoff)
                    .fetch_one(pool)
                    .await?,
            )],
            FAMILY_ALERT_NOTIFICATION => vec![(
                "notification_events".to_owned(),
                sqlx::query_scalar("SELECT COUNT(*) FROM notification_events WHERE created_at < ?")
                    .bind(&cutoff)
                    .fetch_one(pool)
                    .await?,
            )],
            FAMILY_PEER_PRESENCE_INTERVAL => vec![(
                "peer_presence_intervals".to_owned(),
                sqlx::query_scalar("SELECT COUNT(*) FROM peer_presence_intervals WHERE closed_at IS NOT NULL AND closed_at < ?")
                    .bind(&cutoff)
                    .fetch_one(pool)
                    .await?,
            )],
            FAMILY_PEER_AGGREGATE_5M => vec![(
                "peer_aggregate_5m".to_owned(),
                sqlx::query_scalar("SELECT COUNT(*) FROM peer_aggregate_5m WHERE bucket_start < ?")
                    .bind(&cutoff)
                    .fetch_one(pool)
                    .await?,
            )],
            FAMILY_PEER_AGGREGATE_1H => vec![(
                "peer_aggregate_1h".to_owned(),
                sqlx::query_scalar("SELECT COUNT(*) FROM peer_aggregate_1h WHERE bucket_start < ?")
                    .bind(&cutoff)
                    .fetch_one(pool)
                    .await?,
            )],
            _ => Vec::new(),
        };
        for (table, total) in entries {
            plan.push(PlanEntry {
                family: policy.family.clone(),
                table,
                cutoff: cutoff.clone(),
                total,
                deleted: 0,
            });
        }
    }

    for (code, message) in warnings {
        crate::operations::add_warning(state, operation_id, &code, &message).await?;
    }
    Ok(plan)
}

async fn finish_run(
    state: &AppState,
    operation_id: &str,
    mut params: Value,
    plan: Vec<PlanEntry>,
) -> Result<(), crate::operations::OperationError> {
    let mut families: Vec<Value> = Vec::new();
    for entry in plan
        .iter()
        .filter(|entry| entry.deleted > 0 || entry.total > 0)
    {
        if let Some(existing) = families
            .iter_mut()
            .find(|value| value["family"] == entry.family)
        {
            existing["deletedRows"] =
                serde_json::json!(existing["deletedRows"].as_i64().unwrap_or(0) + entry.deleted);
        } else {
            families.push(serde_json::json!({
                "family": entry.family,
                "deletedRows": entry.deleted,
            }));
        }
    }
    params["plan"] = serde_json::to_value(&plan)?;
    let status = if plan.is_empty() {
        crate::operations::STATUS_SUCCEEDED
    } else {
        // Preserve warnings recorded during planning (unsupported/disabled
        // explicit requests) — never plain Success for a partial run.
        let warnings: i64 = sqlx::query_scalar(
            "SELECT json_array_length(warnings_json) FROM operations WHERE operation_id = ?",
        )
        .bind(operation_id)
        .fetch_one(state.db().pool())
        .await?;
        if warnings > 0 {
            crate::operations::STATUS_SUCCEEDED_WITH_WARNINGS
        } else {
            crate::operations::STATUS_SUCCEEDED
        }
    };
    crate::operations::finalize(
        state,
        operation_id,
        status,
        Some(&serde_json::json!({ "families": families })),
        &["retention"],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_is_bounded_and_preserves_dedup_state_in_temp_sqlite() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let old =
            crate::auth::format_rfc3339(raw_block_summary_cutoff(now) - time::Duration::hours(1));
        let fresh = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('retention-agent', 1, ?, ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('retention-network', 'Retention', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('retention-node', 'retention-agent', 'retention-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
        let insert = "INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('retention-node', ?, '0xhash', '0xparent', '0xgenesis', 1, 1, 'lat', 1, ?, 2, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', ?);";
        for height in 0..(RAW_BLOCK_SUMMARY_CLEANUP_BATCH + 5) {
            sqlx::query(insert)
                .bind(height)
                .bind(&old)
                .bind(&old)
                .execute(pool)
                .await
                .unwrap();
        }
        sqlx::query(insert)
            .bind(9_999_i64)
            .bind(&fresh)
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES ('retention-node', 9999, 133, 266, 0, ?)")
            .bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ('retention-node', 0, 9999, 'covered', ?, ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, created_at) VALUES ('retention-node', 100, 110, 'permanent_gap', ?)")
            .bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_identity_window (node_id, height, block_hash, retained_until, observed_at) VALUES ('retention-node', 9999, '0xhash', ?, ?)")
            .bind(crate::auth::format_rfc3339(now + time::Duration::days(30))).bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO chain_divergence_observations (node_id, height, retained_block_hash, observed_block_hash, observed_at, reason, retained_observed_at) VALUES ('retention-node', 9999, '0xhash', '0xother', ?, 'test', ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();

        assert_eq!(
            cleanup_raw_block_summaries(pool, now).await.unwrap(),
            RAW_BLOCK_SUMMARY_CLEANUP_BATCH as u64
        );
        assert_eq!(cleanup_raw_block_summaries(pool, now).await.unwrap(), 5);
        assert_eq!(cleanup_raw_block_summaries(pool, now).await.unwrap(), 0);
        let fresh_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE block_number=9999")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(fresh_count, 1);
        let preserved: (i64, i64, i64, i64) = sqlx::query_as("SELECT historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count FROM block_history_state WHERE node_id='retention-node'").fetch_one(pool).await.unwrap();
        assert_eq!(preserved, (9999, 133, 266, 0));
        for table in [
            "block_coverage_intervals",
            "block_history_gaps",
            "block_identity_window",
            "chain_divergence_observations",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .unwrap();
            assert_eq!(count, 1, "retention deleted preservation table {table}");
        }
    }

    #[tokio::test]
    async fn policy_bounds_and_validation_follow_the_catalog() {
        assert_eq!(validate_policy_days("raw_block_summary", 7), Ok(()));
        assert!(validate_policy_days("raw_block_summary", 0).is_err());
        assert!(validate_policy_days("raw_block_summary", 31).is_err());
        assert_eq!(validate_policy_days("history_gap", 0), Ok(()));
        assert!(validate_policy_days("history_gap", 179).is_err());
        assert_eq!(validate_policy_days("one_hour_aggregate", 0), Ok(()));
        assert!(validate_policy_days("one_hour_aggregate", 30).is_err());
        assert_eq!(
            validate_policy_days(FAMILY_PEER_PRESENCE_INTERVAL, 30),
            Ok(())
        );
        assert!(validate_policy_days(FAMILY_PEER_PRESENCE_INTERVAL, 0).is_err());
        assert!(validate_policy_days(FAMILY_PEER_PRESENCE_INTERVAL, 366).is_err());
        assert!(validate_policy_days("unknown_family", 7).is_err());
    }

    #[tokio::test]
    async fn peer_presence_retention_deletes_only_old_closed_intervals() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let now_text = crate::auth::format_rfc3339(now);
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('peer-retention-network', 'Peer Retention', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('peer-retention-agent', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('peer-retention-node', 'peer-retention-agent', 'peer-retention-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let insert = "INSERT INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, client_name, opened_at, closed_at) VALUES ('peer-retention-node', ?, 'inbound', 1, 0, 1, 'PlatON/v1.5.1', ?, ?)";
        sqlx::query(insert)
            .bind("peer-old")
            .bind("2020-01-01T00:00:00Z")
            .bind("2020-01-02T00:00:00Z")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert)
            .bind("peer-open")
            .bind("2020-01-01T00:00:00Z")
            .bind(None::<String>)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert)
            .bind("peer-fresh")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        ensure_seeded(pool).await.unwrap();
        let operation_id = "peer-retention-operation";
        sqlx::query("INSERT INTO operations (operation_id, kind, status, request_id, params_json, warnings_json, errors_json, created_at) VALUES (?, 'retention_run', 'queued', 'peer-retention-request', ?, '[]', '[]', ?)")
            .bind(operation_id)
            .bind(serde_json::json!({"families": [FAMILY_PEER_PRESENCE_INTERVAL]}).to_string())
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let state = AppState::new(
            database,
            None,
            crate::auth::AuthConfig::development(
                crate::secrets::load_pepper_file(&pepper_path).unwrap(),
                "http://127.0.0.1:8080".to_owned(),
            ),
        );

        execute_step(&state, operation_id).await.unwrap();
        execute_step(&state, operation_id).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM operations WHERE operation_id=?",)
                .bind(operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            crate::operations::STATUS_SUCCEEDED
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE peer_id='peer-old'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE peer_id='peer-open'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "retention never deletes open intervals"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE peer_id='peer-fresh'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "retention preserves intervals inside the configured window"
        );
    }

    #[tokio::test]
    async fn aggregate_retention_deletes_old_five_minute_rows_and_keeps_hourly_forever() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let now_text = crate::auth::format_rfc3339(now);
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('aggregate-retention-network', 'Aggregate Retention', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('aggregate-retention-agent', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('aggregate-retention-node', 'aggregate-retention-agent', 'aggregate-retention-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let old = "2020-01-01T00:00:00Z";
        let insert_5m = "INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, first_observed_at, last_observed_at) VALUES ('aggregate-retention-node', ?, 1, 1, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0, ?, ?)";
        sqlx::query(insert_5m)
            .bind(old)
            .bind(old)
            .bind(old)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert_5m)
            .bind(&now_text)
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO peer_aggregate_5m_countries (node_id, bucket_start, country_code, peer_count) VALUES ('aggregate-retention-node', ?, 'US', 1)")
            .bind(old)
            .execute(pool)
            .await
            .unwrap();
        let insert_1h = "INSERT INTO peer_aggregate_1h (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, first_observed_at, last_observed_at) VALUES ('aggregate-retention-node', ?, 1, 1, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0, ?, ?)";
        sqlx::query(insert_1h)
            .bind(old)
            .bind(old)
            .bind(old)
            .execute(pool)
            .await
            .unwrap();
        ensure_seeded(pool).await.unwrap();
        let operation_id = "aggregate-retention-operation";
        sqlx::query("INSERT INTO operations (operation_id, kind, status, request_id, params_json, warnings_json, errors_json, created_at) VALUES (?, 'retention_run', 'queued', 'aggregate-retention-request', ?, '[]', '[]', ?)")
            .bind(operation_id)
            .bind(serde_json::json!({"families": [FAMILY_PEER_AGGREGATE_5M, FAMILY_PEER_AGGREGATE_1H]}).to_string())
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let state = AppState::new(
            database,
            None,
            crate::auth::AuthConfig::development(
                crate::secrets::load_pepper_file(&pepper_path).unwrap(),
                "http://127.0.0.1:8080".to_owned(),
            ),
        );

        execute_step(&state, operation_id).await.unwrap();
        execute_step(&state, operation_id).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM operations WHERE operation_id=?")
                .bind(operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            crate::operations::STATUS_SUCCEEDED
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_5m WHERE bucket_start=?"
            )
            .bind(old)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_5m_countries WHERE bucket_start=?"
            )
            .bind(old)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "country rows are removed by the aggregate foreign-key cascade"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_5m WHERE bucket_start=?"
            )
            .bind(&now_text)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_1h WHERE bucket_start=?"
            )
            .bind(old)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "the configured hourly zero-day policy keeps long-term rows"
        );
    }

    #[tokio::test]
    async fn seeding_is_idempotent_and_impact_counts_only_old_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        ensure_seeded(pool).await.unwrap();
        ensure_seeded(pool).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM retention_policies")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(count, POLICY_CATALOG.len() as i64);

        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let (estimated, unsupported) = estimate_impact(pool, "one_minute_aggregate", 90, now)
            .await
            .unwrap();
        assert!(unsupported);
        assert_eq!(estimated, 0);
        let (estimated, unsupported) = estimate_impact(pool, "raw_block_summary", 7, now)
            .await
            .unwrap();
        assert!(!unsupported);
        assert_eq!(estimated, 0);
    }
}
