//! Typed Alert Rules, evaluation state machine, Incidents, Silence, and
//! Maintenance (design §17, issue #48).
//!
//! Rules are Server-owned typed conditions over accepted projection facts.
//! Agents never create Alerts; the Server evaluates. Evaluation state and
//! Incident history are persisted in the same SQLite transactions that
//! accept reports, so timers survive restarts and Unknown/Stale inputs can
//! never silently resolve an Open Incident. Silence and Maintenance are
//! time-bounded policies that suppress delivery/marking without changing
//! evaluation facts.

use serde::{Deserialize, Serialize};

use std::time::Duration;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::auth::{format_rfc3339, now_utc, parse_rfc3339};

/// Cadence of the background evaluation sweep. Report ingestion also
/// evaluates the affected subjects in its own transaction.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// The typed catalog of rule subjects (design §17.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Agent,
    Host,
    Node,
    Network,
    Validator,
    Server,
}

impl SubjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SubjectKind::Agent => "agent",
            SubjectKind::Host => "host",
            SubjectKind::Node => "node",
            SubjectKind::Network => "network",
            SubjectKind::Validator => "validator",
            SubjectKind::Server => "server",
        }
    }

    pub fn parse_str(value: &str) -> Option<Self> {
        match value {
            "agent" => Some(SubjectKind::Agent),
            "host" => Some(SubjectKind::Host),
            "node" => Some(SubjectKind::Node),
            "network" => Some(SubjectKind::Network),
            "validator" => Some(SubjectKind::Validator),
            "server" => Some(SubjectKind::Server),
            _ => None,
        }
    }
}

/// Allowed rule severities in severity order.
pub const SEVERITIES: &[&str] = &["info", "warning", "critical"];

pub fn severity_rank(severity: &str) -> u8 {
    match severity {
        "info" => 0,
        "warning" => 1,
        "critical" => 2,
        _ => 0,
    }
}

/// Typed per-rule condition parameters (design §17.1: `for`, recovery
/// threshold/hysteresis, and an optional typed threshold). `for_secs` is
/// the sustained firing duration before an Incident opens;
/// `recovery_for_secs` is the sustained fresh recovery duration before it
/// resolves. Boolean-fact rules fix their internal threshold at 0.5 and do
/// not accept a user threshold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct RuleCondition {
    pub for_secs: u64,
    pub recovery_for_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
}

impl RuleCondition {
    pub const fn boolean(for_secs: u64, recovery_for_secs: u64) -> Self {
        Self {
            for_secs,
            recovery_for_secs,
            threshold: None,
        }
    }

    pub const fn with_threshold(for_secs: u64, recovery_for_secs: u64, threshold: f64) -> Self {
        Self {
            for_secs,
            recovery_for_secs,
            threshold: Some(threshold),
        }
    }

    /// The numeric threshold used for `Known(value)` comparisons. Boolean
    /// facts map to 1.0/0.0 with an internal 0.5 threshold.
    pub fn effective_threshold(&self) -> f64 {
        self.threshold.unwrap_or(0.5)
    }
}

/// Editor schema for one condition parameter, sent to the WebUI so the
/// typed rule form renders per-rule fields (no free-form DSL ever).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParamSchema {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub description: &'static str,
}

/// One typed catalog entry (design §17.1). The key is the immutable rule
/// identity; parameters are validated against `params` at the trust
/// boundary.
pub struct RuleDefinition {
    pub key: &'static str,
    pub subject_kind: SubjectKind,
    pub default_severity: &'static str,
    pub default_condition: RuleCondition,
    /// Optional configurable threshold schema (None = boolean-fact rule).
    pub threshold_param: Option<ParamSchema>,
}

const fn seconds_param(default: f64) -> ParamSchema {
    ParamSchema {
        key: "for_secs",
        label: "Sustained firing",
        unit: "s",
        min: 1.0,
        max: 604_800.0,
        default,
        description: "How long the condition must hold before an Incident opens.",
    }
}

const fn recovery_param(default: f64) -> ParamSchema {
    ParamSchema {
        key: "recovery_for_secs",
        label: "Sustained recovery",
        unit: "s",
        min: 1.0,
        max: 604_800.0,
        default,
        description: "How long fresh recovery must hold before an Incident resolves.",
    }
}

const fn threshold_param(
    label: &'static str,
    unit: &'static str,
    min: f64,
    max: f64,
    default: f64,
) -> ParamSchema {
    ParamSchema {
        key: "threshold",
        label,
        unit,
        min,
        max,
        default,
        description: "The typed threshold this rule compares its fact against.",
    }
}

/// The first rule catalog (design §17.1). `host.*` rules use the Agent's
/// host as subject (host observations are collected once per Agent).
pub static CATALOG: &[RuleDefinition] = &[
    RuleDefinition {
        key: "agent.offline",
        subject_kind: SubjectKind::Agent,
        default_severity: "warning",
        default_condition: RuleCondition::with_threshold(60, 120, 120.0),
        threshold_param: Some(threshold_param("Offline after", "s", 10.0, 86_400.0, 120.0)),
    },
    RuleDefinition {
        key: "node.rpc_unreachable",
        subject_kind: SubjectKind::Node,
        default_severity: "warning",
        default_condition: RuleCondition::boolean(60, 120),
        threshold_param: None,
    },
    RuleDefinition {
        key: "node.head_subscription_disconnected",
        subject_kind: SubjectKind::Node,
        default_severity: "warning",
        default_condition: RuleCondition::with_threshold(60, 120, 180.0),
        threshold_param: Some(threshold_param("Silent after", "s", 10.0, 86_400.0, 180.0)),
    },
    RuleDefinition {
        key: "node.observation_stale",
        subject_kind: SubjectKind::Node,
        default_severity: "warning",
        default_condition: RuleCondition::with_threshold(60, 120, 120.0),
        threshold_param: Some(threshold_param("Stale after", "s", 10.0, 86_400.0, 120.0)),
    },
    RuleDefinition {
        key: "node.process_not_running",
        subject_kind: SubjectKind::Node,
        default_severity: "critical",
        default_condition: RuleCondition::boolean(30, 120),
        threshold_param: None,
    },
    RuleDefinition {
        key: "node.block_stalled",
        subject_kind: SubjectKind::Node,
        default_severity: "critical",
        default_condition: RuleCondition::with_threshold(120, 180, 10.0),
        threshold_param: Some(threshold_param("Lag", "blocks", 1.0, 10_000.0, 10.0)),
    },
    RuleDefinition {
        key: "node.sync_lag",
        subject_kind: SubjectKind::Node,
        default_severity: "warning",
        default_condition: RuleCondition::with_threshold(120, 180, 10.0),
        threshold_param: Some(threshold_param("Lag", "blocks", 1.0, 10_000.0, 10.0)),
    },
    RuleDefinition {
        key: "node.network_identity_mismatch",
        subject_kind: SubjectKind::Node,
        default_severity: "critical",
        default_condition: RuleCondition::boolean(30, 120),
        threshold_param: None,
    },
    RuleDefinition {
        key: "node.consensus_stalled",
        subject_kind: SubjectKind::Node,
        default_severity: "warning",
        default_condition: RuleCondition::boolean(120, 180),
        threshold_param: None,
    },
    RuleDefinition {
        key: "host.disk_pressure",
        subject_kind: SubjectKind::Host,
        default_severity: "warning",
        default_condition: RuleCondition::with_threshold(120, 300, 90.0),
        threshold_param: Some(threshold_param("Usage", "%", 1.0, 100.0, 90.0)),
    },
    RuleDefinition {
        key: "host.memory_pressure",
        subject_kind: SubjectKind::Host,
        default_severity: "warning",
        default_condition: RuleCondition::with_threshold(120, 300, 90.0),
        threshold_param: Some(threshold_param("Usage", "%", 1.0, 100.0, 90.0)),
    },
];

pub fn catalog_rule(key: &str) -> Option<&'static RuleDefinition> {
    CATALOG.iter().find(|rule| rule.key == key)
}

/// Build the editor schema for a rule: the common duration params plus the
/// rule's optional typed threshold.
pub fn rule_schema(rule: &RuleDefinition) -> Vec<ParamSchema> {
    let mut schema = vec![
        seconds_param(rule.default_condition.for_secs as f64),
        recovery_param(rule.default_condition.recovery_for_secs as f64),
    ];
    if let Some(param) = &rule.threshold_param {
        schema.push(param.clone());
    }
    schema
}

/// Validate a condition against the typed catalog entry. Rejects unknown
/// parameters (serde `deny_unknown_fields`), out-of-range durations, and
/// thresholds the rule does not define or that leave the allowed range.
pub fn validate_condition(rule_key: &str, condition: &RuleCondition) -> Result<(), String> {
    let Some(rule) = catalog_rule(rule_key) else {
        return Err(format!("unknown alert rule `{rule_key}`"));
    };
    if !(1..=604_800).contains(&condition.for_secs) {
        return Err("`for_secs` must be between 1 and 604800".to_owned());
    }
    if !(1..=604_800).contains(&condition.recovery_for_secs) {
        return Err("`recovery_for_secs` must be between 1 and 604800".to_owned());
    }
    match (&rule.threshold_param, condition.threshold) {
        (None, Some(_)) => Err(format!(
            "rule `{rule_key}` is a boolean-fact rule and accepts no `threshold`"
        )),
        (Some(param), Some(value)) => {
            if (param.min..=param.max).contains(&value) {
                Ok(())
            } else {
                Err(format!(
                    "`threshold` must be between {} and {} {}",
                    param.min, param.max, param.unit
                ))
            }
        }
        (Some(_), None) => Err(format!("rule `{rule_key}` requires a `threshold`")),
        (None, None) => Ok(()),
    }
}

/// Insert the typed catalog into a fresh database. Idempotent: existing
/// rows (possibly Owner-edited) are never overwritten.
pub async fn seed_catalog(executor: &mut sqlx::SqliteConnection) -> Result<(), sqlx::Error> {
    let created = format_rfc3339(now_utc());
    for rule in CATALOG {
        let condition =
            serde_json::to_string(&rule.default_condition).expect("catalog conditions serialize");
        sqlx::query(
            "INSERT OR IGNORE INTO alert_rules (rule_key, enabled, severity, version, condition_json, created_at, updated_at) VALUES (?, 1, ?, 1, ?, ?, ?)",
        )
        .bind(rule.key)
        .bind(rule.default_severity)
        .bind(&condition)
        .bind(&created)
        .bind(&created)
        .execute(&mut *executor)
        .await?;
        // The initial seed is version 1 and belongs in the immutable
        // version history so Incident retention can always be traced.
        sqlx::query(
            "INSERT OR IGNORE INTO alert_rule_versions (rule_key, version, severity, condition_json, created_at) VALUES (?, 1, ?, ?, ?)",
        )
        .bind(rule.key)
        .bind(rule.default_severity)
        .bind(&condition)
        .bind(&created)
        .execute(&mut *executor)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Evaluation input
// ---------------------------------------------------------------------------

/// The typed evaluation input (design §17.3). `Known(value)` facts are
/// compared against the rule threshold by the state machine; `Unknown` and
/// `Stale` never mean recovered; `Unsupported` never alerts.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvalInput {
    Known {
        value: f64,
        detail: String,
    },
    Unknown {
        reason: String,
    },
    Stale {
        value: f64,
        age_secs: i64,
        detail: String,
    },
    Unsupported {
        reason: String,
    },
}

impl EvalInput {
    pub fn kind_str(&self) -> &'static str {
        match self {
            EvalInput::Known { .. } => "known",
            EvalInput::Unknown { .. } => "unknown",
            EvalInput::Stale { .. } => "stale",
            EvalInput::Unsupported { .. } => "unsupported",
        }
    }

    pub fn value(&self) -> Option<f64> {
        match self {
            EvalInput::Known { value, .. } | EvalInput::Stale { value, .. } => Some(*value),
            _ => None,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            EvalInput::Known { detail, .. } => detail,
            EvalInput::Unknown { reason } => reason,
            EvalInput::Stale { detail, .. } => detail,
            EvalInput::Unsupported { reason } => reason,
        }
    }

    /// Whether this input fires against a threshold. Only `Known` values
    /// are compared (design §17.3).
    pub fn fires(&self, threshold: f64) -> bool {
        match self {
            EvalInput::Known { value, .. } => *value >= threshold,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Persisted evaluation state and the state machine
// ---------------------------------------------------------------------------

/// The persisted evaluation state row for one `(rule_key, subject_key)`.
#[derive(Debug, Clone)]
pub struct RuleState {
    pub state: String,
    pub since: String,
    pub pending_since: Option<String>,
    pub firing_since: Option<String>,
    pub recovering_since: Option<String>,
    pub input_kind: String,
    pub input_value: Option<f64>,
    pub input_detail: Option<String>,
    pub evidence_json: Option<String>,
    pub evaluation_unavailable: bool,
    pub last_evaluated_at: String,
}

fn elapsed_secs(since: &str, now: OffsetDateTime) -> Option<i64> {
    let since = parse_rfc3339(since)?;
    Some((now - since).whole_seconds())
}

/// The outcome of one state-machine transition, used both by the writer
/// (`evaluate_rule`) and the read-only preview.
#[derive(Debug, Clone)]
pub struct Transition {
    pub state: String,
    pub since: String,
    pub pending_since: Option<String>,
    pub firing_since: Option<String>,
    pub recovering_since: Option<String>,
    pub evaluation_unavailable: bool,
    /// `true` when this transition opens a new Incident sequence.
    pub opens_incident: bool,
    /// `true` when this transition resolves the Open Incident.
    pub resolves_incident: bool,
    /// Human-readable transition note for preview and audit.
    pub note: String,
}

/// The state machine (design §17.2/§17.3):
///
/// ```text
/// Normal → Pending → Firing → Recovering → Normal
/// ```
///
/// - `Known` firing sustained for `for_secs` opens an Incident;
/// - `Known` recovered sustained for `recovery_for_secs` resolves it;
/// - Unknown/Stale never resolve an Open Incident (it stays Open and
///   evaluation is marked unavailable); timers restart so only fresh known
///   evidence can open or resolve;
/// - Unsupported never alerts and never changes state.
pub fn project_transition(
    state: &RuleState,
    input: &EvalInput,
    condition: &RuleCondition,
    now: OffsetDateTime,
) -> Transition {
    let now_text = format_rfc3339(now);
    let threshold = condition.effective_threshold();
    let mut transition = Transition {
        state: state.state.clone(),
        since: state.since.clone(),
        pending_since: state.pending_since.clone(),
        firing_since: state.firing_since.clone(),
        recovering_since: state.recovering_since.clone(),
        evaluation_unavailable: state.evaluation_unavailable,
        opens_incident: false,
        resolves_incident: false,
        note: String::new(),
    };

    match input {
        EvalInput::Unsupported { .. } => {
            // Unsupported input is not fresh Known evidence: it never fires
            // or resolves, and it restarts every timer so a later Known
            // interval cannot borrow evidence from before the unsupported
            // period (continuous-evidence contract).
            transition.pending_since = None;
            transition.firing_since = None;
            transition.recovering_since = None;
            transition.note = "unsupported input restarts evaluation timers".to_owned();
            return transition;
        }
        EvalInput::Known { .. } => {
            let firing = input.fires(threshold);
            transition.evaluation_unavailable = false;
            match transition.state.as_str() {
                "normal" | "pending" => {
                    if firing {
                        let pending_since = match transition.pending_since {
                            Some(since) if transition.state == "pending" => since,
                            _ => now_text.clone(),
                        };
                        let sustained = elapsed_secs(&pending_since, now).unwrap_or(0)
                            >= condition.for_secs as i64;
                        if sustained {
                            transition.state = "firing".to_owned();
                            transition.since = now_text.clone();
                            transition.pending_since = None;
                            transition.firing_since = Some(now_text.clone());
                            transition.opens_incident = true;
                            transition.note = format!(
                                "firing sustained for {}s; Incident opens",
                                condition.for_secs
                            );
                        } else {
                            transition.state = "pending".to_owned();
                            transition.since = pending_since.clone();
                            transition.pending_since = Some(pending_since);
                            transition.note =
                                "condition firing; waiting for sustained duration".to_owned();
                        }
                    } else {
                        transition.state = "normal".to_owned();
                        transition.since = now_text.clone();
                        transition.pending_since = None;
                        transition.note = "condition recovered before Incident opened".to_owned();
                    }
                }
                "firing" => {
                    if firing {
                        transition.note = "condition still firing".to_owned();
                    } else {
                        transition.state = "recovering".to_owned();
                        transition.since = now_text.clone();
                        transition.recovering_since = Some(now_text.clone());
                        transition.note =
                            "condition recovered; waiting for sustained recovery".to_owned();
                    }
                }
                "recovering" => {
                    if firing {
                        transition.state = "firing".to_owned();
                        transition.since = now_text.clone();
                        transition.firing_since = Some(now_text.clone());
                        transition.recovering_since = None;
                        // The original Incident is still open: re-firing during
                        // recovery must not create a second open Incident.
                        transition.note =
                            "condition re-fired during recovery; Incident stays open".to_owned();
                    } else {
                        let sustained = transition
                            .recovering_since
                            .as_deref()
                            .and_then(|since| elapsed_secs(since, now))
                            .unwrap_or(0)
                            >= condition.recovery_for_secs as i64;
                        if sustained {
                            transition.state = "normal".to_owned();
                            transition.since = now_text.clone();
                            transition.recovering_since = None;
                            transition.resolves_incident = true;
                            transition.note = format!(
                                "recovery sustained for {}s; Incident resolves",
                                condition.recovery_for_secs
                            );
                        } else {
                            transition.note = "recovery in progress".to_owned();
                        }
                    }
                }
                _ => {}
            }
        }
        EvalInput::Unknown { .. } | EvalInput::Stale { .. } => {
            // Unknown/Stale never means recovered and never silently
            // resolves; an Open Incident stays Open and evaluation is
            // marked unavailable. Timers only advance on fresh Known
            // inputs, so stale evidence cannot open or resolve Incidents.
            match transition.state.as_str() {
                "pending" => {
                    transition.state = "normal".to_owned();
                    transition.since = now_text.clone();
                    transition.pending_since = None;
                    transition.note =
                        "input unknown/stale; pending timer reset, condition not confirmed"
                            .to_owned();
                }
                "firing" => {
                    transition.evaluation_unavailable = true;
                    transition.note = "input unknown/stale; Open Incident stays open".to_owned();
                }
                "recovering" => {
                    transition.recovering_since = Some(now_text.clone());
                    transition.since = now_text.clone();
                    transition.note =
                        "input unknown/stale; recovery timer restarts on fresh known evidence"
                            .to_owned();
                }
                _ => {
                    transition.note = "input unknown/stale; evaluation unchanged".to_owned();
                }
            }
        }
    }
    transition
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors surfaced by alert evaluation and administration.
#[derive(Debug, thiserror::Error)]
pub enum AlertError {
    #[error("alert database query failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    Validation(String),
}

impl AlertError {
    /// Map an evaluation error onto the HTTP error envelope (the report
    /// ingestion transaction and the Admin API share this mapping).
    pub fn status(&self) -> axum::http::StatusCode {
        match self {
            AlertError::Validation(_) => axum::http::StatusCode::BAD_REQUEST,
            AlertError::Database(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            AlertError::Validation(_) => "alert_validation",
            AlertError::Database(_) => "unavailable",
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }
}

// ---------------------------------------------------------------------------
// Effective rule resolution (global default + Network/Node overrides)
// ---------------------------------------------------------------------------

/// The effective rule after Network/Node override resolution (design
/// §17.1). Node overrides win over Network overrides; unset override fields
/// inherit from the global rule.
#[derive(Debug, Clone)]
pub struct EffectiveRule {
    pub rule_key: String,
    pub enabled: bool,
    pub severity: String,
    pub condition: RuleCondition,
    pub version: i64,
}

/// Resolve the effective rule for one subject. Node subjects resolve their
/// Network override through the node's registered Network; Host/Agent
/// subjects only have the global rule.
pub async fn effective_rule(
    executor: &mut sqlx::SqliteConnection,
    rule_key: &str,
    subject_kind: SubjectKind,
    subject_key: &str,
) -> Result<Option<EffectiveRule>, sqlx::Error> {
    let row = sqlx::query_as::<_, (bool, String, i64, String)>(
        "SELECT enabled, severity, version, condition_json FROM alert_rules WHERE rule_key = ?",
    )
    .bind(rule_key)
    .fetch_optional(&mut *executor)
    .await?;
    let Some((enabled, severity, version, condition_json)) = row else {
        return Ok(None);
    };
    let mut enabled = enabled;
    let mut severity = severity;
    let mut condition: RuleCondition = match serde_json::from_str(&condition_json) {
        Ok(condition) => condition,
        Err(_) => return Ok(None),
    };

    // Network override applies to Node subjects only; Node override wins.
    let network_key = if subject_kind == SubjectKind::Node {
        sqlx::query_scalar::<_, Option<String>>("SELECT network_key FROM nodes WHERE node_id = ?")
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?
            .flatten()
    } else {
        None
    };
    if let Some(network_key) = network_key {
        if let Some((override_enabled, override_severity, override_condition_json)) =
            sqlx::query_as::<_, (Option<bool>, Option<String>, Option<String>)>(
                "SELECT enabled, severity, condition_json FROM alert_rule_overrides WHERE rule_key = ? AND scope_kind = 'network' AND scope_value = ?",
            )
            .bind(rule_key)
            .bind(&network_key)
            .fetch_optional(&mut *executor)
            .await?
        {
            apply_override(
                &mut enabled,
                &mut severity,
                &mut condition,
                override_enabled,
                override_severity,
                override_condition_json.as_deref(),
            );
        }
    }
    if subject_kind == SubjectKind::Node {
        if let Some((override_enabled, override_severity, override_condition_json)) =
            sqlx::query_as::<_, (Option<bool>, Option<String>, Option<String>)>(
                "SELECT enabled, severity, condition_json FROM alert_rule_overrides WHERE rule_key = ? AND scope_kind = 'node' AND scope_value = ?",
            )
            .bind(rule_key)
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?
        {
            apply_override(
                &mut enabled,
                &mut severity,
                &mut condition,
                override_enabled,
                override_severity,
                override_condition_json.as_deref(),
            );
        }
    }

    Ok(Some(EffectiveRule {
        rule_key: rule_key.to_owned(),
        enabled,
        severity,
        condition,
        version,
    }))
}

fn apply_override(
    enabled: &mut bool,
    severity: &mut String,
    condition: &mut RuleCondition,
    override_enabled: Option<bool>,
    override_severity: Option<String>,
    override_condition_json: Option<&str>,
) {
    if let Some(value) = override_enabled {
        *enabled = value;
    }
    if let Some(value) = override_severity {
        *severity = value;
    }
    if let Some(json) = override_condition_json {
        if let Ok(value) = serde_json::from_str::<RuleCondition>(json) {
            *condition = value;
        }
    }
}

// ---------------------------------------------------------------------------
// Input extraction from accepted projection facts
// ---------------------------------------------------------------------------

fn age_secs(received_at: Option<&str>, now: OffsetDateTime) -> Option<i64> {
    let received_at = parse_rfc3339(received_at?)?;
    Some((now - received_at).whole_seconds())
}

fn known(value: f64, detail: String) -> EvalInput {
    EvalInput::Known { value, detail }
}

/// Server-owned staleness bound for evaluation facts (design §17.3: only
/// fresh Known evidence opens or resolves Incidents). Follows the current
/// `node.observation_stale` threshold so Owners tune one number; defaults
/// to 120s when the rule is missing or unparseable.
pub(crate) async fn freshness_bound(
    executor: &mut sqlx::SqliteConnection,
) -> Result<i64, sqlx::Error> {
    let json: Option<String> = sqlx::query_scalar(
        "SELECT condition_json FROM alert_rules WHERE rule_key = 'node.observation_stale'",
    )
    .fetch_optional(&mut *executor)
    .await?;
    let Some(json) = json else {
        return Ok(120);
    };
    let Ok(condition) = serde_json::from_str::<RuleCondition>(&json) else {
        return Ok(120);
    };
    Ok(condition.threshold.map(|t| t as i64).unwrap_or(120).max(1))
}

/// Extract the typed evaluation fact for one `(rule, subject)` from the
/// accepted Current Projection (design §17.3). Every rule maps onto honest
/// projection facts; missing facts are `Unknown`, never zero/Healthy.
pub async fn extract_input(
    executor: &mut sqlx::SqliteConnection,

    rule_key: &str,
    subject_kind: SubjectKind,
    subject_key: &str,
    now: OffsetDateTime,
    stale_after_secs: i64,
) -> Result<EvalInput, sqlx::Error> {
    match (subject_kind, rule_key) {
        (SubjectKind::Agent, "agent.offline") => {
            let last = sqlx::query_scalar::<_, Option<String>>(
                "SELECT MAX(received_at) FROM agent_report_receipts WHERE agent_id = ?",
            )
            .bind(subject_key)
            .fetch_one(&mut *executor)
            .await?;
            match last {
                Some(received_at) => {
                    let age = age_secs(Some(&received_at), now).unwrap_or(i64::MAX);
                    Ok(known(
                        age as f64,
                        format!("last report {}s ago", if age == i64::MAX { 0 } else { age }),
                    ))
                }
                None => Ok(EvalInput::Unknown {
                    reason: "Agent never reported".to_owned(),
                }),
            }
        }
        (SubjectKind::Node, "node.rpc_unreachable") => {
            let row = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>)>(
                "SELECT state, error_code, error_message, received_at FROM component_status WHERE node_id = ? AND component_key = 'rpc'",
            )
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?;
            Ok(match row {
                Some(row) => component_input(row, now, stale_after_secs),
                None => EvalInput::Unknown {
                    reason: "RPC never observed".to_owned(),
                },
            })
        }
        (SubjectKind::Node, "node.head_subscription_disconnected") => {
            let last = sqlx::query_scalar::<_, Option<String>>(
                "SELECT updated_at FROM block_history_state WHERE node_id = ?",
            )
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?;
            match last.flatten() {
                Some(updated_at) => {
                    let age = age_secs(Some(&updated_at), now).unwrap_or(i64::MAX);
                    Ok(known(
                        age as f64,
                        format!("block stream silent for {}s", age),
                    ))
                }
                None => Ok(EvalInput::Unknown {
                    reason: "No block history observed".to_owned(),
                }),
            }
        }
        (SubjectKind::Node, "node.observation_stale") => {
            let last = sqlx::query_scalar::<_, Option<String>>(
                "SELECT MAX(received_at) FROM component_status WHERE node_id = ?",
            )
            .bind(subject_key)
            .fetch_one(&mut *executor)
            .await?;
            match last {
                Some(received_at) => {
                    let age = age_secs(Some(&received_at), now).unwrap_or(i64::MAX);
                    Ok(known(age as f64, format!("last observation {}s ago", age)))
                }
                None => Ok(EvalInput::Unknown {
                    reason: "Node never observed".to_owned(),
                }),
            }
        }
        (SubjectKind::Node, "node.process_not_running") => {
            let row = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>)>(
                "SELECT state, error_code, error_message, received_at FROM component_status WHERE node_id = ? AND component_key = 'process'",
            )
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?;
            Ok(match row {
                Some(row) => component_input(row, now, stale_after_secs),
                None => EvalInput::Unknown {
                    reason: "Process never observed".to_owned(),
                },
            })
        }
        (SubjectKind::Node, "node.block_stalled") => {
            let row = sqlx::query_as::<_, (Option<i64>, Option<String>, Option<String>)>(
                "SELECT h.current_head, n.network_key, h.updated_at FROM block_history_state h JOIN nodes n ON n.node_id = h.node_id WHERE h.node_id = ?",
            )
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?;
            let Some((node_head, network_key, updated_at)) = row else {
                return Ok(EvalInput::Unknown {
                    reason: "No block history observed".to_owned(),
                });
            };
            if let Some(age) = age_secs(updated_at.as_deref(), now) {
                if age > stale_after_secs {
                    return Ok(EvalInput::Stale {
                        value: 0.0,
                        age_secs: age,
                        detail: format!("block head evidence is {age}s old"),
                    });
                }
            }
            let Some(network_key) = network_key else {
                return Ok(EvalInput::Unknown {
                    reason: "Node has no registered Network".to_owned(),
                });
            };
            let Some(node_head) = node_head else {
                return Ok(EvalInput::Unknown {
                    reason: "No node head observed".to_owned(),
                });
            };
            let reference_head: Option<i64> = sqlx::query_scalar::<_, Option<i64>>(
                "SELECT block_number FROM network_reference_heads WHERE network_key = ?",
            )
            .bind(network_key)
            .fetch_optional(&mut *executor)
            .await?
            .flatten();
            match reference_head {
                Some(reference_head) => {
                    let lag = reference_head.saturating_sub(node_head);
                    Ok(known(
                        lag as f64,
                        format!(
                            "node head {node_head} is {lag} blocks behind the observed reference head {reference_head}"
                        ),
                    ))
                }
                None => Ok(EvalInput::Unknown {
                    reason: "No Network reference head observed".to_owned(),
                }),
            }
        }
        (SubjectKind::Node, "node.sync_lag") => {
            let row = sqlx::query_as::<_, SyncRow>(
                "SELECT s.state, c.syncing, c.current_block, c.highest_block, s.received_at FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = ? AND s.component_key = 'sync'",
            )
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?;
            Ok(sync_lag_input(row, now, stale_after_secs))
        }
        (SubjectKind::Node, "node.network_identity_mismatch") => {
            network_identity_input(executor, subject_key).await
        }
        (SubjectKind::Node, "node.consensus_stalled") => {
            let row = sqlx::query_as::<_, ConsensusRow>(
                "SELECT s.state, s.error_code, s.error_message, c.consensus_epoch, c.consensus_view_number, c.consensus_highest_commit_block, s.received_at FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = ? AND s.component_key = 'consensus'",
            )
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?;
            Ok(consensus_input(row, now, stale_after_secs))
        }
        (SubjectKind::Host, "host.disk_pressure") => {
            let row = sqlx::query_as::<_, (Option<f64>, Option<String>)>(
                "SELECT MAX(used_bytes * 1.0 / total_bytes), MAX(updated_at) FROM current_host_disk_mounts WHERE agent_id = ?",
            )
            .bind(subject_key)
            .fetch_one(&mut *executor)
            .await?;
            match row {
                (Some(usage), updated_at) => {
                    if let Some(age) = age_secs(updated_at.as_deref(), now) {
                        if age > stale_after_secs {
                            return Ok(EvalInput::Stale {
                                value: 0.0,
                                age_secs: age,
                                detail: format!("disk observation is {age}s old"),
                            });
                        }
                    }
                    let percent = usage * 100.0;
                    Ok(known(
                        percent,
                        format!("worst mount at {percent:.1}% usage"),
                    ))
                }
                _ => Ok(EvalInput::Unknown {
                    reason: "No disk mounts observed".to_owned(),
                }),
            }
        }
        (SubjectKind::Host, "host.memory_pressure") => {
            let row = sqlx::query_as::<_, (Option<i64>, Option<i64>, Option<String>)>(
                "SELECT memory_used_bytes, memory_total_bytes, updated_at FROM current_host_observations WHERE agent_id = ?",
            )
            .bind(subject_key)
            .fetch_optional(&mut *executor)
            .await?;
            match row {
                Some((Some(used), Some(total), updated_at)) if total > 0 => {
                    if let Some(age) = age_secs(updated_at.as_deref(), now) {
                        if age > stale_after_secs {
                            return Ok(EvalInput::Stale {
                                value: 0.0,
                                age_secs: age,
                                detail: format!("memory observation is {age}s old"),
                            });
                        }
                    }
                    let percent = used as f64 / total as f64 * 100.0;
                    Ok(known(percent, format!("memory at {percent:.1}% usage")))
                }
                _ => Ok(EvalInput::Unknown {
                    reason: "No memory observation".to_owned(),
                }),
            }
        }
        _ => Ok(EvalInput::Unsupported {
            reason: format!("rule `{rule_key}` has no v1 input mapping for this subject"),
        }),
    }
}

fn component_input(
    row: (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    now: OffsetDateTime,
    stale_after_secs: i64,
) -> EvalInput {
    let age = age_secs(row.3.as_deref(), now);
    if let Some(age) = age {
        if age > stale_after_secs {
            // Preserve last-good semantics: the stored state is retained but
            // stale evidence is never fresh recovery (and never fires).
            return EvalInput::Stale {
                value: 0.0,
                age_secs: age,
                detail: format!("observation is {age}s old"),
            };
        }
    }
    match row.0.as_deref() {
        Some("error") => known(
            1.0,
            format!(
                "{}: {}",
                row.1.unwrap_or_else(|| "component_error".to_owned()),
                row.2.unwrap_or_else(|| "component failed".to_owned())
            ),
        ),
        Some("ok") => known(0.0, "component ok".to_owned()),
        Some("starting") => EvalInput::Unknown {
            reason: "component still starting".to_owned(),
        },
        Some("disabled") => EvalInput::Unsupported {
            reason: "component disabled".to_owned(),
        },
        Some("unsupported") => EvalInput::Unsupported {
            reason: "component unsupported".to_owned(),
        },
        _ => EvalInput::Unknown {
            reason: "component never observed".to_owned(),
        },
    }
}

type SyncRow = (
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

fn sync_lag_input(row: Option<SyncRow>, now: OffsetDateTime, stale_after_secs: i64) -> EvalInput {
    match row {
        Some((state, syncing, current, highest, received_at)) => {
            if let Some(age) = age_secs(received_at.as_deref(), now) {
                if age > stale_after_secs {
                    return EvalInput::Stale {
                        value: 0.0,
                        age_secs: age,
                        detail: format!("sync observation is {age}s old"),
                    };
                }
            }
            match state.as_deref() {
                Some("ok") => {
                    if let (Some(current), Some(highest)) = (current, highest) {
                        let lag = highest.saturating_sub(current);
                        known(
                            lag as f64,
                            format!(
                                "node head {current} is {lag} blocks behind its observed highest block {highest}"
                            ),
                        )
                    } else if syncing == Some(1) {
                        // The node declares it is synchronizing but exposes no
                        // head values; there is no measurable lag to compare.
                        known(1.0, "node reports it is synchronizing".to_owned())
                    } else {
                        known(0.0, "sync observation without head values".to_owned())
                    }
                }
                Some("error") => EvalInput::Unknown {
                    reason: "sync probe failed".to_owned(),
                },
                Some("starting") => EvalInput::Unknown {
                    reason: "sync probe starting".to_owned(),
                },
                Some("disabled") | Some("unsupported") => EvalInput::Unsupported {
                    reason: "sync probe not supported".to_owned(),
                },
                _ => EvalInput::Unknown {
                    reason: "sync never observed".to_owned(),
                },
            }
        }
        None => EvalInput::Unknown {
            reason: "sync never observed".to_owned(),
        },
    }
}

type ConsensusRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

fn consensus_input(
    row: Option<ConsensusRow>,
    now: OffsetDateTime,
    stale_after_secs: i64,
) -> EvalInput {
    match row {
        Some((state, error_code, error_message, epoch, view, commit, received_at)) => {
            if let Some(age) = age_secs(received_at.as_deref(), now) {
                if age > stale_after_secs {
                    return EvalInput::Stale {
                        value: 0.0,
                        age_secs: age,
                        detail: format!("consensus observation is {age}s old"),
                    };
                }
            }
            match state.as_deref() {
                Some("error") => known(
                    1.0,
                    format!(
                        "{}: {}",
                        error_code.unwrap_or_else(|| "consensus_error".to_owned()),
                        error_message.unwrap_or_else(|| "consensus probe failed".to_owned())
                    ),
                ),
                Some("ok") => known(
                    0.0,
                    format!(
                        "consensus ok (epoch {}, view {}, highest commit {})",
                        epoch.unwrap_or(-1),
                        view.unwrap_or(-1),
                        commit.unwrap_or(-1)
                    ),
                ),
                Some("starting") => EvalInput::Unknown {
                    reason: "consensus probe starting".to_owned(),
                },
                Some("disabled") | Some("unsupported") => EvalInput::Unsupported {
                    reason: "consensus probe not supported".to_owned(),
                },
                _ => EvalInput::Unknown {
                    reason: "consensus never observed".to_owned(),
                },
            }
        }
        None => EvalInput::Unknown {
            reason: "consensus never observed".to_owned(),
        },
    }
}

/// Compare the node's observed Network Identity tuple against the
/// registered Network (mirrors the Admin identity disposition; issue #45).
/// Only a complete four-field observation can match or mismatch; partial
/// observations stay `unknown`.
async fn network_identity_input(
    executor: &mut sqlx::SqliteConnection,

    subject_key: &str,
) -> Result<EvalInput, sqlx::Error> {
    let observed = sqlx::query_as::<_, (Option<String>, Option<i64>, Option<i64>, Option<String>)>(
        "SELECT network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp FROM current_node_chain_observations WHERE node_id = ?",
    )
    .bind(subject_key)
    .fetch_optional(&mut *executor)
    .await?;
    let expected = sqlx::query_as::<_, (Option<String>, Option<i64>, Option<i64>, Option<String>)>(
        "SELECT n.genesis_hash, n.chain_id, n.p2p_network_id, n.address_hrp FROM nodes x JOIN networks n ON n.network_key = x.network_key WHERE x.node_id = ?",
    )
    .bind(subject_key)
    .fetch_optional(&mut *executor)
    .await?;
    match (observed, expected) {
        (
            Some((Some(genesis), Some(chain), Some(p2p), Some(hrp))),
            Some((Some(exp_genesis), Some(exp_chain), Some(exp_p2p), Some(exp_hrp))),
        ) => {
            let mut mismatches = Vec::new();
            if genesis != exp_genesis {
                mismatches.push("genesis hash");
            }
            if chain != exp_chain {
                mismatches.push("chain id");
            }
            if p2p != exp_p2p {
                mismatches.push("p2p network id");
            }
            if hrp != exp_hrp {
                mismatches.push("address hrp");
            }
            if mismatches.is_empty() {
                Ok(known(
                    0.0,
                    "observed identity matches the registered Network".to_owned(),
                ))
            } else {
                Ok(known(
                    1.0,
                    format!(
                        "observed identity mismatches the registered Network: {}",
                        mismatches.join(", ")
                    ),
                ))
            }
        }
        _ => Ok(EvalInput::Unknown {
            reason: "identity partially observed or Network unregistered".to_owned(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Evaluation (write path)
// ---------------------------------------------------------------------------

/// Result of evaluating one rule for one subject.
#[derive(Debug, Clone, Default)]
pub struct EvaluationOutcome {
    pub changed: bool,
    pub opened_incident: Option<String>,
    pub resolved_incident: Option<bool>,
    pub state: String,
}

async fn load_state(
    executor: &mut sqlx::SqliteConnection,
    rule_key: &str,
    subject_key: &str,
) -> Result<Option<RuleState>, sqlx::Error> {
    load_state_inner(executor, rule_key, subject_key).await
}

pub(crate) async fn load_state_public(
    executor: &mut sqlx::SqliteConnection,
    rule_key: &str,
    subject_key: &str,
) -> Result<Option<RuleState>, sqlx::Error> {
    load_state_inner(executor, rule_key, subject_key).await
}

async fn load_state_inner(
    executor: &mut sqlx::SqliteConnection,
    rule_key: &str,
    subject_key: &str,
) -> Result<Option<RuleState>, sqlx::Error> {
    let row = sqlx::query_as::<_, (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<f64>,
        Option<String>,
        Option<String>,
        bool,
        String,
    )>(
        "SELECT state, since, pending_since, firing_since, recovering_since, input_kind, input_value, input_detail, evidence_json, evaluation_unavailable, last_evaluated_at FROM alert_rule_state WHERE rule_key = ? AND subject_key = ?",
    )
    .bind(rule_key)
    .bind(subject_key)
    .fetch_optional(&mut *executor)
    .await?;
    Ok(row.map(
        |(
            state,
            since,
            pending_since,
            firing_since,
            recovering_since,
            input_kind,
            input_value,
            input_detail,
            evidence_json,
            evaluation_unavailable,
            last_evaluated_at,
        )| RuleState {
            state,
            since,
            pending_since,
            firing_since,
            recovering_since,
            input_kind,
            input_value,
            input_detail,
            evidence_json,
            evaluation_unavailable,
            last_evaluated_at,
        },
    ))
}

/// Evaluate one `(rule, subject)` and persist the resulting state machine
/// transition and any Incident open/resolve. Disabled rules are skipped and
/// their persisted state is left untouched (history is never deleted).
pub async fn evaluate_rule(
    executor: &mut sqlx::SqliteConnection,

    rule_key: &str,
    subject_kind: SubjectKind,
    subject_key: &str,
    now: OffsetDateTime,
) -> Result<EvaluationOutcome, AlertError> {
    let mut outcome = EvaluationOutcome::default();
    let Some(effective) = effective_rule(executor, rule_key, subject_kind, subject_key).await?
    else {
        return Ok(outcome);
    };
    if !effective.enabled {
        return Ok(outcome);
    }
    let stale_after_secs = freshness_bound(&mut *executor).await?;
    let input = extract_input(
        executor,
        rule_key,
        subject_kind,
        subject_key,
        now,
        stale_after_secs,
    )
    .await?;
    let state = match load_state(executor, rule_key, subject_key).await? {
        Some(state) => state,
        None => RuleState {
            state: "normal".to_owned(),
            since: format_rfc3339(now),
            pending_since: None,
            firing_since: None,
            recovering_since: None,
            input_kind: "known".to_owned(),
            input_value: None,
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(now),
        },
    };
    let transition = project_transition(&state, &input, &effective.condition, now);
    outcome.state = transition.state.clone();
    outcome.changed = transition.state != state.state
        || transition.evaluation_unavailable != state.evaluation_unavailable;

    if transition.opens_incident {
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM alert_incidents WHERE rule_key = ? AND subject_key = ?",
        )
        .bind(rule_key)
        .bind(subject_key)
        .fetch_one(&mut *executor)
        .await?;
        let incident_id = uuid::Uuid::new_v4().to_string();
        let evidence = evidence_json(&input, &effective, &transition, now);
        let opened_at = format_rfc3339(now);
        sqlx::query(
            "INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES (?, ?, ?, ?, ?, ?, 'open', ?, ?, ?)",
        )
        .bind(&incident_id)
        .bind(rule_key)
        .bind(effective.version)
        .bind(subject_kind.as_str())
        .bind(subject_key)
        .bind(&effective.severity)
        .bind(sequence)
        .bind(&opened_at)
        .bind(&evidence)
        .execute(&mut *executor)
        .await?;
        outcome.changed = true;
        outcome.opened_incident = Some(incident_id);
    }
    if transition.resolves_incident {
        let resolved_at = format_rfc3339(now);
        let evidence = evidence_json(&input, &effective, &transition, now);
        let result = sqlx::query(
            "UPDATE alert_incidents SET state = 'resolved', resolved_at = ?, resolved_evidence_json = ? WHERE rule_key = ? AND subject_key = ? AND state = 'open'",
        )
        .bind(&resolved_at)
        .bind(&evidence)
        .bind(rule_key)
        .bind(subject_key)
        .execute(&mut *executor)
        .await?;
        if result.rows_affected() > 0 {
            outcome.changed = true;
            outcome.resolved_incident = Some(true);
        }
    }

    // Persist the state row (upsert).
    let now_text = format_rfc3339(now);
    let (input_kind, input_value, input_detail) = match &input {
        EvalInput::Known { value, detail } => ("known", Some(*value), Some(detail.clone())),
        EvalInput::Unknown { reason } => ("unknown", None, Some(reason.clone())),
        EvalInput::Stale { value, detail, .. } => ("stale", Some(*value), Some(detail.clone())),
        EvalInput::Unsupported { reason } => ("unsupported", None, Some(reason.clone())),
    };
    let evidence = evidence_json(&input, &effective, &transition, now);
    sqlx::query(
        "INSERT INTO alert_rule_state (rule_key, subject_kind, subject_key, state, since, pending_since, firing_since, recovering_since, input_kind, input_value, input_detail, evidence_json, evaluation_unavailable, last_evaluated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(rule_key, subject_key) DO UPDATE SET state=excluded.state, since=excluded.since, pending_since=excluded.pending_since, firing_since=excluded.firing_since, recovering_since=excluded.recovering_since, input_kind=excluded.input_kind, input_value=excluded.input_value, input_detail=excluded.input_detail, evidence_json=excluded.evidence_json, evaluation_unavailable=excluded.evaluation_unavailable, last_evaluated_at=excluded.last_evaluated_at",
    )
    .bind(rule_key)
    .bind(subject_kind.as_str())
    .bind(subject_key)
    .bind(&transition.state)
    .bind(&transition.since)
    .bind(&transition.pending_since)
    .bind(&transition.firing_since)
    .bind(&transition.recovering_since)
    .bind(input_kind)
    .bind(input_value)
    .bind(input_detail)
    .bind(&evidence)
    .bind(transition.evaluation_unavailable as i64)
    .bind(&now_text)
    .execute(&mut *executor)
    .await?;
    Ok(outcome)
}

fn evidence_json(
    input: &EvalInput,
    effective: &EffectiveRule,
    transition: &Transition,
    now: OffsetDateTime,
) -> String {
    let evidence = serde_json::json!({
        "input_kind": input.kind_str(),
        "input_value": input.value(),
        "input_detail": input.detail(),
        "firing": input.fires(effective.condition.effective_threshold()),
        "threshold": effective.condition.effective_threshold(),
        "rule_severity": effective.severity,
        "state": transition.state,
        "note": transition.note,
        "evaluated_at": format_rfc3339(now),
    });
    evidence.to_string()
}

/// Evaluate the subjects of one accepted Agent report. Called inside the
/// report ingestion transaction: the Agent's offline/host rules and every
/// reported Node's rules run on the freshly committed projection facts.
/// Returns the number of state transitions.
pub async fn evaluate_report(
    executor: &mut sqlx::SqliteConnection,

    agent_id: &str,
    node_ids: &[String],
    now: OffsetDateTime,
) -> Result<usize, AlertError> {
    let mut changes = 0usize;
    for rule in CATALOG {
        match rule.subject_kind {
            SubjectKind::Agent | SubjectKind::Host => {
                let outcome =
                    evaluate_rule(executor, rule.key, rule.subject_kind, agent_id, now).await?;
                changes += outcome.changed as usize;
            }
            SubjectKind::Node => {
                for node_id in node_ids {
                    let outcome =
                        evaluate_rule(executor, rule.key, SubjectKind::Node, node_id, now).await?;
                    changes += outcome.changed as usize;
                }
            }
            _ => {}
        }
    }
    Ok(changes)
}

/// The full evaluation sweep: every catalog rule against every active
/// subject. Runs as a bounded background task (design §17.2: Server
/// restart restores timers from persisted state; the sweep applies current
/// facts immediately).
pub async fn sweep(state: &crate::http::AppState) -> Result<usize, AlertError> {
    let Some(_guard) = state.ingestion_guard() else {
        return Ok(0);
    };
    let now = now_utc();
    let mut tx = state.db().pool().begin().await?;
    let mut changes = 0usize;

    let agents: Vec<String> = sqlx::query_scalar("SELECT agent_id FROM agents")
        .fetch_all(&mut *tx)
        .await?;
    let nodes: Vec<String> =
        sqlx::query_scalar("SELECT node_id FROM nodes WHERE lifecycle = 'active'")
            .fetch_all(&mut *tx)
            .await?;

    for rule in CATALOG {
        match rule.subject_kind {
            SubjectKind::Agent | SubjectKind::Host => {
                for agent_id in &agents {
                    let outcome =
                        evaluate_rule(&mut tx, rule.key, rule.subject_kind, agent_id, now).await?;
                    changes += outcome.changed as usize;
                }
            }
            SubjectKind::Node => {
                for node_id in &nodes {
                    let outcome =
                        evaluate_rule(&mut tx, rule.key, SubjectKind::Node, node_id, now).await?;
                    changes += outcome.changed as usize;
                }
            }
            _ => {}
        }
    }
    tx.commit().await?;
    Ok(changes)
}

// ---------------------------------------------------------------------------
// Suppression matching (Silence + Maintenance)
// ---------------------------------------------------------------------------

/// One active suppression policy matching a subject.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuppressionMatch {
    pub kind: &'static str,
    pub id: String,
    pub reason: String,
    pub starts_at: String,
    pub ends_at: String,
    /// Maintenance-only: whether this match marks the Incident suppressed
    /// (design §17.5). Silences never mark Incidents.
    pub marks_incident: bool,
}

/// Subject context needed to resolve Agent/Network matchers for Node
/// subjects.
struct SubjectContext {
    agent_id: Option<String>,
    network_key: Option<String>,
}

async fn subject_context(
    executor: &mut sqlx::SqliteConnection,

    subject_kind: SubjectKind,
    subject_key: &str,
) -> Result<SubjectContext, sqlx::Error> {
    let (agent_id, network_key) = if subject_kind == SubjectKind::Node {
        let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT agent_id, network_key FROM nodes WHERE node_id = ?",
        )
        .bind(subject_key)
        .fetch_optional(&mut *executor)
        .await?;
        match row {
            Some((agent_id, network_key)) => (agent_id, network_key),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    Ok(SubjectContext {
        agent_id,
        network_key,
    })
}

fn silence_matches(
    matcher_kind: &str,
    matcher_value: Option<&str>,
    subject_kind: SubjectKind,
    subject_key: &str,
    context: &SubjectContext,
) -> bool {
    match matcher_kind {
        "all" => true,
        "agent" => match subject_kind {
            SubjectKind::Agent | SubjectKind::Host => Some(subject_key) == matcher_value,
            SubjectKind::Node => context.agent_id.as_deref() == matcher_value,
            _ => false,
        },
        "node" => subject_kind == SubjectKind::Node && Some(subject_key) == matcher_value,
        "network" => {
            subject_kind == SubjectKind::Node && context.network_key.as_deref() == matcher_value
        }
        _ => false,
    }
}

fn maintenance_matches(
    scope_kind: &str,
    scope_value: &str,
    subject_kind: SubjectKind,
    subject_key: &str,
    context: &SubjectContext,
) -> bool {
    match scope_kind {
        "agent" => match subject_kind {
            SubjectKind::Agent | SubjectKind::Host => subject_key == scope_value,
            SubjectKind::Node => context.agent_id.as_deref() == Some(scope_value),
            _ => false,
        },
        "node" => subject_kind == SubjectKind::Node && subject_key == scope_value,
        "network" => {
            subject_kind == SubjectKind::Node && context.network_key.as_deref() == Some(scope_value)
        }
        _ => false,
    }
}

/// All active suppression matches (Silence + Maintenance) for one subject
/// (design §17.5). Silence suppresses delivery only; Maintenance also marks
/// expected offline/process/RPC Incidents suppressed. Both reasons stay
/// visible independently when they overlap.
pub async fn suppressions_for_subject(
    executor: &mut sqlx::SqliteConnection,
    rule_key: &str,
    subject_kind: SubjectKind,
    subject_key: &str,
    now: OffsetDateTime,
) -> Result<Vec<SuppressionMatch>, sqlx::Error> {
    let now_text = format_rfc3339(now);
    let context = subject_context(executor, subject_kind, subject_key).await?;
    let mut matches = Vec::new();

    let silences = sqlx::query_as::<_, (String, String, Option<String>, String, String, String)>(
        "SELECT silence_id, matcher_kind, matcher_value, reason, starts_at, ends_at FROM silences WHERE cancelled_at IS NULL AND starts_at <= ? AND ends_at > ? ORDER BY ends_at",
    )
    .bind(&now_text)
    .bind(&now_text)
    .fetch_all(&mut *executor)
    .await?;
    for (id, matcher_kind, matcher_value, reason, starts_at, ends_at) in silences {
        if silence_matches(
            &matcher_kind,
            matcher_value.as_deref(),
            subject_kind,
            subject_key,
            &context,
        ) {
            matches.push(SuppressionMatch {
                kind: "silence",
                id,
                reason,
                starts_at,
                ends_at,
                marks_incident: false,
            });
        }
    }

    let windows = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
        "SELECT window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at FROM maintenance_windows WHERE cancelled_at IS NULL AND starts_at <= ? AND ends_at > ? ORDER BY ends_at",
    )
    .bind(&now_text)
    .bind(&now_text)
    .fetch_all(&mut *executor)
    .await?;
    for (id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at) in windows {
        // Fail closed: corrupted stored JSON never degrades into a wildcard
        // suppression (empty = match any Rule is an explicit Owner choice,
        // not a parse-failure fallback).
        let Ok(expected) = serde_json::from_str::<Vec<String>>(&expected_rule_keys) else {
            continue;
        };
        if !expected.is_empty() && !expected.iter().any(|key| key == rule_key) {
            continue;
        }
        if maintenance_matches(
            &scope_kind,
            &scope_value,
            subject_kind,
            subject_key,
            &context,
        ) {
            matches.push(SuppressionMatch {
                kind: "maintenance",
                id,
                reason,
                starts_at,
                ends_at,
                marks_incident: true,
            });
        }
    }

    matches.sort_by(|a, b| a.ends_at.cmp(&b.ends_at));
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::format_rfc3339;
    use sqlx::SqlitePool;
    use tempfile::tempdir;
    use time::macros::datetime;

    fn base_time() -> OffsetDateTime {
        datetime!(2026-03-01 00:00:00 UTC)
    }

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool().clone();
        (dir, pool)
    }

    async fn seed_subject(pool: &SqlitePool, agent_id: &str, node_id: &str) {
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES (?, 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .bind(agent_id)
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 210425, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, 'mainnet', 'ws://127.0.0.1:1', 'active', 'private', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .bind(node_id)
            .bind(agent_id)
            .execute(pool).await.unwrap();
    }

    async fn set_rpc_state(
        pool: &SqlitePool,
        agent_id: &str,
        node_id: &str,
        state: &str,
        error: Option<(&str, &str)>,
        observed_at: OffsetDateTime,
    ) {
        let now = format_rfc3339(observed_at);
        let (error_code, error_message) = error.unwrap_or(("", ""));
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES (?, 'node', ?, ?, 'rpc', ?, ?, ?, ?, 1, 1, ?, ?) ON CONFLICT(agent_id, scope, scope_key, component_key) DO UPDATE SET state=excluded.state, received_at=excluded.received_at, state_revision=state_revision+1, error_code=excluded.error_code, error_message=excluded.error_message")
            .bind(agent_id)
            .bind(node_id)
            .bind(node_id)
            .bind(state)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(if error.is_some() { error_code } else { "" })
            .bind(if error.is_some() { error_message } else { "" })
            .execute(pool)
            .await
            .unwrap();
    }

    async fn update_condition(pool: &SqlitePool, rule_key: &str, condition: RuleCondition) {
        let json = serde_json::to_string(&condition).unwrap();
        sqlx::query("UPDATE alert_rules SET condition_json = ? WHERE rule_key = ?")
            .bind(json)
            .bind(rule_key)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn count_incidents(pool: &SqlitePool, rule_key: &str, subject_key: &str) -> (i64, i64) {
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM alert_incidents WHERE rule_key = ? AND subject_key = ? AND state = 'open'",
        )
        .bind(rule_key)
        .bind(subject_key)
        .fetch_one(pool)
        .await
        .unwrap();
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM alert_incidents WHERE rule_key = ? AND subject_key = ?",
        )
        .bind(rule_key)
        .bind(subject_key)
        .fetch_one(pool)
        .await
        .unwrap();
        (open, total)
    }

    async fn evaluate(
        pool: &SqlitePool,
        rule_key: &str,
        subject_kind: SubjectKind,
        subject_key: &str,
        now: OffsetDateTime,
    ) -> EvaluationOutcome {
        let mut conn = pool.acquire().await.unwrap();
        evaluate_rule(&mut conn, rule_key, subject_kind, subject_key, now)
            .await
            .unwrap()
    }

    async fn extract(
        pool: &SqlitePool,
        rule_key: &str,
        subject_kind: SubjectKind,
        subject_key: &str,
        now: OffsetDateTime,
    ) -> EvalInput {
        let mut conn = pool.acquire().await.unwrap();
        extract_input(&mut conn, rule_key, subject_kind, subject_key, now, 120)
            .await
            .unwrap()
    }

    async fn effective(
        pool: &SqlitePool,
        rule_key: &str,
        subject_kind: SubjectKind,
        subject_key: &str,
    ) -> EffectiveRule {
        let mut conn = pool.acquire().await.unwrap();
        effective_rule(&mut conn, rule_key, subject_kind, subject_key)
            .await
            .unwrap()
            .expect("rule exists")
    }

    async fn suppressions(
        pool: &SqlitePool,
        rule_key: &str,
        subject_kind: SubjectKind,
        subject_key: &str,
        now: OffsetDateTime,
    ) -> Vec<SuppressionMatch> {
        let mut conn = pool.acquire().await.unwrap();
        suppressions_for_subject(&mut conn, rule_key, subject_kind, subject_key, now)
            .await
            .unwrap()
    }

    async fn report_eval(
        pool: &SqlitePool,
        agent_id: &str,
        node_ids: &[String],
        now: OffsetDateTime,
    ) -> usize {
        let mut conn = pool.acquire().await.unwrap();
        evaluate_report(&mut conn, agent_id, node_ids, now)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn catalog_seeds_all_rules_and_is_idempotent() {
        let (_dir, pool) = test_pool().await;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_rules")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, CATALOG.len() as i64);
        // Re-seeding never overwrites Owner edits.
        sqlx::query(
            "UPDATE alert_rules SET severity = 'critical' WHERE rule_key = 'agent.offline'",
        )
        .execute(&pool)
        .await
        .unwrap();
        {
            let mut conn = pool.acquire().await.unwrap();
            seed_catalog(&mut conn).await.unwrap();
        }
        let severity: String =
            sqlx::query_scalar("SELECT severity FROM alert_rules WHERE rule_key = 'agent.offline'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(severity, "critical");
    }

    #[test]
    fn condition_validation_is_typed_per_rule() {
        // Boolean-fact rules reject a threshold.
        assert!(
            validate_condition(
                "node.rpc_unreachable",
                &RuleCondition::with_threshold(60, 120, 90.0)
            )
            .is_err()
        );
        assert!(
            validate_condition("node.rpc_unreachable", &RuleCondition::boolean(60, 120)).is_ok()
        );
        // Threshold rules require the threshold within bounds.
        assert!(
            validate_condition(
                "host.memory_pressure",
                &RuleCondition::with_threshold(60, 120, 101.0)
            )
            .is_err()
        );
        assert!(
            validate_condition(
                "host.memory_pressure",
                &RuleCondition::with_threshold(60, 120, 90.0)
            )
            .is_ok()
        );
        assert!(
            validate_condition("host.memory_pressure", &RuleCondition::boolean(60, 120)).is_err()
        );
        // Durations are bounded and unknown rules are rejected.
        assert!(
            validate_condition(
                "agent.offline",
                &RuleCondition::with_threshold(0, 120, 120.0)
            )
            .is_err()
        );
        assert!(validate_condition("nope", &RuleCondition::boolean(60, 120)).is_err());
        // Unknown JSON parameters are rejected by serde.
        let bad = r#"{"for_secs":60,"recovery_for_secs":120,"threshold":90.0,"script":"rm -rf"}"#;
        assert!(serde_json::from_str::<RuleCondition>(bad).is_err());
    }

    #[test]
    fn state_machine_opens_and_resolves_incidents() {
        let now = base_time();
        let condition = RuleCondition::with_threshold(60, 120, 90.0);
        let default = RuleState {
            state: "normal".to_owned(),
            since: format_rfc3339(now),
            pending_since: None,
            firing_since: None,
            recovering_since: None,
            input_kind: "known".to_owned(),
            input_value: None,
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(now),
        };

        // Known firing at 95% -> pending.
        let firing = EvalInput::Known {
            value: 95.0,
            detail: "memory at 95.0% usage".to_owned(),
        };
        let t1 = now + time::Duration::seconds(10);
        let transition = project_transition(&default, &firing, &condition, t1);
        assert_eq!(transition.state, "pending");
        assert!(!transition.opens_incident);

        // Sustained firing past `for` -> opens an Incident.
        let pending_state = RuleState {
            state: transition.state.clone(),
            since: transition.since.clone(),
            pending_since: transition.pending_since.clone(),
            firing_since: transition.firing_since.clone(),
            recovering_since: transition.recovering_since.clone(),
            input_kind: "known".to_owned(),
            input_value: Some(95.0),
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(t1),
        };
        let t2 = t1 + time::Duration::seconds(120);
        let transition = project_transition(&pending_state, &firing, &condition, t2);
        assert_eq!(transition.state, "firing");
        assert!(transition.opens_incident);

        // Known recovery -> recovering, then sustained recovery resolves.
        let ok = EvalInput::Known {
            value: 20.0,
            detail: "memory at 20.0% usage".to_owned(),
        };
        let firing_state = RuleState {
            state: "firing".to_owned(),
            since: format_rfc3339(t2),
            pending_since: None,
            firing_since: Some(format_rfc3339(t2)),
            recovering_since: None,
            input_kind: "known".to_owned(),
            input_value: Some(95.0),
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(t2),
        };
        let t3 = t2 + time::Duration::seconds(30);
        let transition = project_transition(&firing_state, &ok, &condition, t3);
        assert_eq!(transition.state, "recovering");
        assert!(!transition.resolves_incident);
        let recovering_state = RuleState {
            state: transition.state.clone(),
            since: transition.since.clone(),
            pending_since: None,
            firing_since: None,
            recovering_since: transition.recovering_since.clone(),
            input_kind: "known".to_owned(),
            input_value: Some(20.0),
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(t3),
        };
        let t4 = t3 + time::Duration::seconds(300);
        let transition = project_transition(&recovering_state, &ok, &condition, t4);
        assert_eq!(transition.state, "normal");
        assert!(transition.resolves_incident);
    }

    #[test]
    fn unknown_and_stale_never_silently_resolve() {
        let now = base_time();
        let condition = RuleCondition::with_threshold(60, 120, 90.0);
        let firing_state = RuleState {
            state: "firing".to_owned(),
            since: format_rfc3339(now),
            pending_since: None,
            firing_since: Some(format_rfc3339(now)),
            recovering_since: None,
            input_kind: "known".to_owned(),
            input_value: Some(95.0),
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(now),
        };
        let unknown = EvalInput::Unknown {
            reason: "probe failed".to_owned(),
        };
        let t = now + time::Duration::hours(1);
        let transition = project_transition(&firing_state, &unknown, &condition, t);
        assert_eq!(transition.state, "firing");
        assert!(transition.evaluation_unavailable);
        assert!(!transition.resolves_incident);

        // Stale during recovery restarts the recovery timer: only fresh
        // Known recovery can resolve.
        let recovering = RuleState {
            state: "recovering".to_owned(),
            since: format_rfc3339(now),
            pending_since: None,
            firing_since: None,
            recovering_since: Some(format_rfc3339(now)),
            input_kind: "known".to_owned(),
            input_value: Some(20.0),
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(now),
        };
        let stale = EvalInput::Stale {
            value: 20.0,
            age_secs: 9999,
            detail: "last value 20.0".to_owned(),
        };
        let t = now + time::Duration::seconds(500);
        let transition = project_transition(&recovering, &stale, &condition, t);
        assert_eq!(transition.state, "recovering");
        assert_eq!(
            transition.recovering_since.as_deref(),
            Some(format_rfc3339(t).as_str())
        );

        // Unsupported never alerts and never changes state.
        let unsupported = EvalInput::Unsupported {
            reason: "component unsupported".to_owned(),
        };
        let transition = project_transition(&firing_state, &unsupported, &condition, t);
        assert_eq!(transition.state, "firing");
        assert!(!transition.evaluation_unavailable);
    }

    #[test]
    fn unsupported_input_restarts_timers_and_refire_keeps_one_incident() {
        let now = base_time();
        let condition = RuleCondition::with_threshold(60, 120, 90.0);

        // Unsupported input must not borrow evidence from before the
        // unsupported interval: every timer restarts.
        let recovering = RuleState {
            state: "recovering".to_owned(),
            since: format_rfc3339(now),
            pending_since: None,
            firing_since: None,
            recovering_since: Some(format_rfc3339(now)),
            input_kind: "known".to_owned(),
            input_value: Some(20.0),
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(now),
        };
        let t = now + time::Duration::seconds(500);
        let unsupported = EvalInput::Unsupported {
            reason: "component unsupported".to_owned(),
        };
        let transition = project_transition(&recovering, &unsupported, &condition, t);
        assert_eq!(transition.state, "recovering");
        assert!(transition.recovering_since.is_none());
        assert!(!transition.resolves_incident);

        // A fresh Known recovery after Unsupported must sustain the FULL
        // recovery duration again before it can resolve.
        let known = EvalInput::Known {
            value: 20.0,
            detail: "component ok".to_owned(),
        };
        let after_unsupported = RuleState {
            state: transition.state.clone(),
            since: transition.since.clone(),
            pending_since: transition.pending_since.clone(),
            firing_since: transition.firing_since.clone(),
            recovering_since: transition.recovering_since.clone(),
            input_kind: "unsupported".to_owned(),
            input_value: None,
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: transition.evaluation_unavailable,
            last_evaluated_at: format_rfc3339(t),
        };
        let t2 = t + time::Duration::seconds(100);
        let transition = project_transition(&after_unsupported, &known, &condition, t2);
        assert_eq!(transition.state, "recovering");
        assert!(!transition.resolves_incident);

        // Re-firing during recovery keeps the original Incident open; it
        // never opens a second one.
        let t3 = t2 + time::Duration::seconds(1);
        let firing = EvalInput::Known {
            value: 95.0,
            detail: "component error".to_owned(),
        };
        let after_known = RuleState {
            state: transition.state.clone(),
            since: transition.since.clone(),
            pending_since: transition.pending_since.clone(),
            firing_since: transition.firing_since.clone(),
            recovering_since: transition.recovering_since.clone(),
            input_kind: "known".to_owned(),
            input_value: Some(20.0),
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: transition.evaluation_unavailable,
            last_evaluated_at: format_rfc3339(t2),
        };
        let transition = project_transition(&after_known, &firing, &condition, t3);
        assert_eq!(transition.state, "firing");
        assert!(!transition.opens_incident);
        assert!(!transition.resolves_incident);
    }

    #[tokio::test]
    async fn stale_component_observations_are_never_fresh_evidence() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        let now = base_time();
        // A very old `ok` observation must NOT count as fresh recovery.
        sqlx::query(
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES ('agent-a', 'node', 'node-a', 'node-a', 'rpc', 'ok', ?, ?, ?, 1, 1)",
        )
        .bind(format_rfc3339(now - time::Duration::hours(2)))
        .bind(format_rfc3339(now - time::Duration::hours(2)))
        .bind(format_rfc3339(now - time::Duration::hours(2)))
        .execute(&pool)
        .await
        .unwrap();
        let input = {
            let mut conn = pool.acquire().await.unwrap();
            extract_input(
                &mut conn,
                "node.rpc_unreachable",
                SubjectKind::Node,
                "node-a",
                now,
                120,
            )
            .await
            .unwrap()
        };
        match input {
            EvalInput::Stale { age_secs, .. } => assert!(age_secs > 120),
            other => panic!("expected Stale input, got {other:?}"),
        }
        // A stale `error` observation likewise never fires: it is evidence
        // of the past, not of a current outage. (The single pool connection
        // must be released before the UPDATE.)
        sqlx::query(
            "UPDATE component_status SET state = 'error', error_code = 'rpc_unreachable', error_message = 'gone' WHERE node_id = 'node-a' AND component_key = 'rpc'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let input = {
            let mut conn = pool.acquire().await.unwrap();
            extract_input(
                &mut conn,
                "node.rpc_unreachable",
                SubjectKind::Node,
                "node-a",
                now,
                120,
            )
            .await
            .unwrap()
        };
        match input {
            EvalInput::Stale { .. } => {}
            other => panic!("expected Stale input, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_expected_rule_keys_fail_closed() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        let now = now_utc();
        let starts = format_rfc3339(now - time::Duration::hours(1));
        let ends = format_rfc3339(now + time::Duration::hours(1));
        sqlx::query(
            "INSERT INTO maintenance_windows (window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at) VALUES ('mnt-bad', 'node', 'node-a', 'not-json[[', 'corrupt', ?, ?, 'owner', ?)",
        )
        .bind(&starts).bind(&ends).bind(&starts)
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let matches = suppressions_for_subject(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            now,
        )
        .await
        .unwrap();
        // Corrupted allowlist JSON must never degrade into "match any Rule".
        assert!(matches.is_empty());
    }

    #[test]
    fn pending_resets_on_unknown_and_requires_fresh_sustained_firing() {
        let now = base_time();
        let condition = RuleCondition::with_threshold(60, 120, 90.0);
        let pending = RuleState {
            state: "pending".to_owned(),
            since: format_rfc3339(now),
            pending_since: Some(format_rfc3339(now)),
            firing_since: None,
            recovering_since: None,
            input_kind: "known".to_owned(),
            input_value: Some(95.0),
            input_detail: None,
            evidence_json: None,
            evaluation_unavailable: false,
            last_evaluated_at: format_rfc3339(now),
        };
        let unknown = EvalInput::Unknown {
            reason: "probe failed".to_owned(),
        };
        let t = now + time::Duration::seconds(1000);
        let transition = project_transition(&pending, &unknown, &condition, t);
        assert_eq!(transition.state, "normal");
        assert!(transition.pending_since.is_none());
    }

    #[tokio::test]
    async fn evaluate_rule_opens_resolves_and_reopens_incidents_with_versions() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        let t0 = base_time();
        set_rpc_state(
            &pool,
            "agent-a",
            "node-a",
            "error",
            Some(("rpc_unreachable", "connect refused")),
            t0,
        )
        .await;
        update_condition(
            &pool,
            "node.rpc_unreachable",
            RuleCondition::boolean(60, 120),
        )
        .await;

        let outcome = evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t0,
        )
        .await;
        assert_eq!(outcome.state, "pending");
        assert!(outcome.opened_incident.is_none());

        // After `for` elapsed the Incident opens with the rule version.
        let t1 = t0 + time::Duration::seconds(61);
        let outcome = evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t1,
        )
        .await;
        assert_eq!(outcome.state, "firing");
        let incident_id = outcome.opened_incident.expect("incident opens");
        let (open, total) = count_incidents(&pool, "node.rpc_unreachable", "node-a").await;
        assert_eq!((open, total), (1, 1));
        let (rule_version, evidence): (i64, String) = sqlx::query_as(
            "SELECT rule_version, opened_evidence_json FROM alert_incidents WHERE incident_id = ?",
        )
        .bind(&incident_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rule_version, 1);
        assert!(evidence.contains("connect refused"));

        // Recovery resolves the Incident; re-firing opens sequence 2. The
        // ok observation is refreshed mid-window so recovery evidence stays
        // fresh for the whole recovery duration (fresh-Known contract).
        set_rpc_state(
            &pool,
            "agent-a",
            "node-a",
            "ok",
            None,
            t1 + time::Duration::seconds(1),
        )
        .await;
        let outcome = evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t1 + time::Duration::seconds(1),
        )
        .await;
        assert_eq!(outcome.state, "recovering");
        set_rpc_state(
            &pool,
            "agent-a",
            "node-a",
            "ok",
            None,
            t1 + time::Duration::seconds(100),
        )
        .await;
        let outcome = evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t1 + time::Duration::seconds(200),
        )
        .await;
        assert_eq!(outcome.state, "normal");
        assert!(outcome.resolved_incident.is_some());
        let (open, total) = count_incidents(&pool, "node.rpc_unreachable", "node-a").await;
        assert_eq!((open, total), (0, 1));
        let resolved: Option<String> =
            sqlx::query_scalar("SELECT resolved_at FROM alert_incidents WHERE incident_id = ?")
                .bind(&incident_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(resolved.is_some());

        set_rpc_state(
            &pool,
            "agent-a",
            "node-a",
            "error",
            Some(("rpc_unreachable", "again")),
            t1 + time::Duration::seconds(300),
        )
        .await;
        let outcome = evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t1 + time::Duration::seconds(300),
        )
        .await;
        assert_eq!(outcome.state, "pending");
        let outcome = evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t1 + time::Duration::seconds(400),
        )
        .await;
        assert_eq!(outcome.state, "firing");
        let (open, total) = count_incidents(&pool, "node.rpc_unreachable", "node-a").await;
        assert_eq!((open, total), (1, 2));
        let seq: i64 = sqlx::query_scalar(
            "SELECT sequence FROM alert_incidents WHERE state = 'open' AND rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(seq, 2);
    }

    #[tokio::test]
    async fn disabled_rule_keeps_history_and_skips_evaluation() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        set_rpc_state(
            &pool,
            "agent-a",
            "node-a",
            "error",
            Some(("rpc_unreachable", "down")),
            base_time(),
        )
        .await;
        update_condition(
            &pool,
            "node.rpc_unreachable",
            RuleCondition::boolean(1, 120),
        )
        .await;

        let t0 = base_time();
        evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t0,
        )
        .await;
        evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t0 + time::Duration::seconds(10),
        )
        .await;
        let (open, total) = count_incidents(&pool, "node.rpc_unreachable", "node-a").await;
        assert_eq!((open, total), (1, 1));

        sqlx::query("UPDATE alert_rules SET enabled = 0 WHERE rule_key = 'node.rpc_unreachable'")
            .execute(&pool)
            .await
            .unwrap();
        let outcome = evaluate(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            t0 + time::Duration::hours(1),
        )
        .await;
        assert!(!outcome.changed);
        // Incident history survives disabling.
        let (open, total) = count_incidents(&pool, "node.rpc_unreachable", "node-a").await;
        assert_eq!((open, total), (1, 1));
        let state: String = sqlx::query_scalar(
            "SELECT state FROM alert_rule_state WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "firing");
    }

    #[tokio::test]
    async fn node_override_wins_over_network_override_and_global() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        let now = format_rfc3339(base_time());
        sqlx::query("INSERT INTO alert_rule_overrides (rule_key, scope_kind, scope_value, enabled, severity, condition_json, created_at, updated_at) VALUES ('host.memory_pressure', 'network', 'mainnet', NULL, NULL, ?, ?, ?)")
            .bind(serde_json::to_string(&RuleCondition::with_threshold(60, 120, 95.0)).unwrap())
            .bind(&now)
            .bind(&now)
            .execute(&pool)
            .await
            .unwrap();
        // The override exists for a network rule but host.memory_pressure is
        // a Host rule: network overrides must not leak into Host subjects.
        let eff = effective(&pool, "host.memory_pressure", SubjectKind::Host, "agent-a").await;
        assert_eq!(eff.condition.threshold, Some(90.0));

        // Node rule with network override then a stricter node override.
        sqlx::query("INSERT INTO alert_rule_overrides (rule_key, scope_kind, scope_value, severity, condition_json, created_at, updated_at) VALUES ('node.rpc_unreachable', 'network', 'mainnet', NULL, ?, ?, ?)")
            .bind(serde_json::to_string(&RuleCondition::boolean(10, 20)).unwrap())
            .bind(&now)
            .bind(&now)
            .execute(&pool)
            .await
            .unwrap();
        let eff = effective(&pool, "node.rpc_unreachable", SubjectKind::Node, "node-a").await;
        assert_eq!(eff.condition.for_secs, 10);

        sqlx::query("INSERT INTO alert_rule_overrides (rule_key, scope_kind, scope_value, enabled, condition_json, created_at, updated_at) VALUES ('node.rpc_unreachable', 'node', 'node-a', 0, NULL, ?, ?)")
            .bind(&now)
            .bind(&now)
            .execute(&pool)
            .await
            .unwrap();
        let eff = effective(&pool, "node.rpc_unreachable", SubjectKind::Node, "node-a").await;
        assert!(!eff.enabled);
        assert_eq!(eff.condition.for_secs, 10); // node override inherits network params
    }

    #[tokio::test]
    async fn agent_and_host_facts_are_honest() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        let now = base_time();

        // Never-reported Agent is Unknown, never firing.
        let input = extract(&pool, "agent.offline", SubjectKind::Agent, "agent-a", now).await;
        assert_eq!(input.kind_str(), "unknown");

        // A report older than the offline threshold fires; a fresh one does not.
        sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES ('r1', 'agent-a', 1, 'boot-a', 1, 'hash', 'accepted', x'00', ?)")
            .bind(format_rfc3339(now - time::Duration::hours(2)))
            .execute(&pool)
            .await
            .unwrap();
        let input = extract(&pool, "agent.offline", SubjectKind::Agent, "agent-a", now).await;
        assert_eq!(input.kind_str(), "known");
        assert!(input.fires(120.0));

        sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES ('r2', 'agent-a', 1, 'boot-a', 2, 'hash2', 'accepted', x'00', ?)")
            .bind(format_rfc3339(now - time::Duration::seconds(5)))
            .execute(&pool)
            .await
            .unwrap();
        let input = extract(&pool, "agent.offline", SubjectKind::Agent, "agent-a", now).await;
        assert!(!input.fires(120.0));

        // Host memory pressure maps the worst observed usage.
        sqlx::query("INSERT INTO current_host_observations (agent_id, memory_used_bytes, memory_total_bytes, updated_at) VALUES ('agent-a', 950, 1000, ?)")
            .bind(format_rfc3339(now))
            .execute(&pool)
            .await
            .unwrap();
        let input = extract(
            &pool,
            "host.memory_pressure",
            SubjectKind::Host,
            "agent-a",
            now,
        )
        .await;
        assert_eq!(input.value(), Some(95.0));
        assert!(input.fires(90.0));

        // No mounts -> Unknown, never a fabricated healthy zero.
        let input = extract(
            &pool,
            "host.disk_pressure",
            SubjectKind::Host,
            "agent-a",
            now,
        )
        .await;
        assert_eq!(input.kind_str(), "unknown");
    }

    #[tokio::test]
    async fn block_and_sync_rules_use_observed_network_reference() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        let now = base_time();
        let now_text = format_rfc3339(now);
        sqlx::query("INSERT INTO block_history_state (node_id, updated_at, current_head) VALUES ('node-a', ?, 100)")
            .bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        // Without a reference head the input is Unknown (never fires).
        let input = extract(
            &pool,
            "node.block_stalled",
            SubjectKind::Node,
            "node-a",
            now,
        )
        .await;
        assert_eq!(input.kind_str(), "unknown");

        sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence) VALUES ('mainnet', 500, ?, 'high')")
            .bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        let input = extract(
            &pool,
            "node.block_stalled",
            SubjectKind::Node,
            "node-a",
            now,
        )
        .await;
        assert_eq!(input.value(), Some(400.0));
        assert!(input.fires(10.0));

        // Sync lag: syncing=1 fires; syncing=0 with a small gap does not.
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, syncing, current_block, highest_block, updated_at) VALUES ('node-a', 1, 100, 500, ?)")
            .bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES ('agent-a', 'node', 'node-a', 'node-a', 'sync', 'ok', ?, ?, ?, 1, 1)")
            .bind(&now_text).bind(&now_text).bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        let input = extract(&pool, "node.sync_lag", SubjectKind::Node, "node-a", now).await;
        assert!(input.fires(10.0));
    }

    #[tokio::test]
    async fn identity_mismatch_fires_only_on_complete_contradiction() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        let now = base_time();
        let now_text = format_rfc3339(now);

        // Partial observation stays unknown.
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_genesis_hash, updated_at) VALUES ('node-a', '0xgenesis', ?)")
            .bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        let input = extract(
            &pool,
            "node.network_identity_mismatch",
            SubjectKind::Node,
            "node-a",
            now,
        )
        .await;
        assert_eq!(input.kind_str(), "unknown");

        // Complete mismatch fires with the contradicting fields named.
        sqlx::query("UPDATE current_node_chain_observations SET network_chain_id = 999, network_p2p_network_id = 2, network_address_hrp = 'lat' WHERE node_id = 'node-a'")
            .execute(&pool)
            .await
            .unwrap();
        let input = extract(
            &pool,
            "node.network_identity_mismatch",
            SubjectKind::Node,
            "node-a",
            now,
        )
        .await;
        assert!(input.fires(0.5));
        assert!(input.detail().contains("chain id"));

        // Complete match is known-not-firing.
        sqlx::query("UPDATE current_node_chain_observations SET network_chain_id = 210425, network_p2p_network_id = 1 WHERE node_id = 'node-a'")
            .execute(&pool)
            .await
            .unwrap();
        let input = extract(
            &pool,
            "node.network_identity_mismatch",
            SubjectKind::Node,
            "node-a",
            now,
        )
        .await;
        assert!(!input.fires(0.5));
    }

    #[tokio::test]
    async fn suppressions_are_scoped_overlapping_and_typed() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        let now = base_time();
        let now_text = format_rfc3339(now);
        let later = format_rfc3339(now + time::Duration::hours(2));
        sqlx::query("INSERT INTO silences (silence_id, matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at) VALUES ('sil-all', 'all', NULL, 'quiet', ?, ?, 'owner', ?)")
            .bind(&now_text).bind(&later).bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO silences (silence_id, matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at) VALUES ('sil-other-node', 'node', 'node-b', 'other', ?, ?, 'owner', ?)")
            .bind(&now_text).bind(&later).bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO maintenance_windows (window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at) VALUES ('mnt-node-a', 'node', 'node-a', '[\"node.rpc_unreachable\"]', 'planned', ?, ?, 'owner', ?)")
            .bind(&now_text).bind(&later).bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        // Maintenance window for a different rule must not match.
        sqlx::query("INSERT INTO maintenance_windows (window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at) VALUES ('mnt-other-rule', 'node', 'node-a', '[\"node.consensus_stalled\"]', 'other', ?, ?, 'owner', ?)")
            .bind(&now_text).bind(&later).bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        let matches = suppressions(
            &pool,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            now,
        )
        .await;
        // sil-all (silence) + mnt-node-a (maintenance) match; the other-node
        // silence and the other-rule window do not.
        let kinds: Vec<(&str, bool)> = matches.iter().map(|m| (m.kind, m.marks_incident)).collect();
        assert!(kinds.contains(&("silence", false)));
        assert!(kinds.contains(&("maintenance", true)));
        assert_eq!(matches.len(), 2);

        // Agent-subject suppression: an agent-scoped maintenance window and
        // an agent silence match Host subjects by their Agent.
        sqlx::query("INSERT INTO silences (silence_id, matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at) VALUES ('sil-agent', 'agent', 'agent-a', 'agent quiet', ?, ?, 'owner', ?)")
            .bind(&now_text).bind(&later).bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO maintenance_windows (window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at) VALUES ('mnt-agent', 'agent', 'agent-a', '[]', 'agent mnt', ?, ?, 'owner', ?)")
            .bind(&now_text).bind(&later).bind(&now_text)
            .execute(&pool)
            .await
            .unwrap();
        let matches = suppressions(
            &pool,
            "host.memory_pressure",
            SubjectKind::Host,
            "agent-a",
            now,
        )
        .await;
        assert_eq!(matches.len(), 3); // sil-all + sil-agent + mnt-agent

        // Expired and cancelled policies never match: the window ends in
        // the past, so it is no longer active.
        let past_start = format_rfc3339(now - time::Duration::hours(4));
        let past_end = format_rfc3339(now - time::Duration::hours(1));
        sqlx::query(
            "UPDATE silences SET starts_at = ?, ends_at = ? WHERE silence_id = 'sil-agent'",
        )
        .bind(&past_start)
        .bind(&past_end)
        .execute(&pool)
        .await
        .unwrap();
        let matches = suppressions(
            &pool,
            "host.memory_pressure",
            SubjectKind::Host,
            "agent-a",
            now,
        )
        .await;
        // sil-all and mnt-agent remain; the expired sil-agent no longer matches.
        assert_eq!(matches.len(), 2);
    }

    #[tokio::test]
    async fn evaluate_report_covers_agent_host_and_node_subjects() {
        let (_dir, pool) = test_pool().await;
        seed_subject(&pool, "agent-a", "node-a").await;
        set_rpc_state(
            &pool,
            "agent-a",
            "node-a",
            "error",
            Some(("rpc_unreachable", "down")),
            base_time(),
        )
        .await;
        update_condition(
            &pool,
            "node.rpc_unreachable",
            RuleCondition::boolean(1, 120),
        )
        .await;
        sqlx::query("INSERT INTO current_host_observations (agent_id, memory_used_bytes, memory_total_bytes, updated_at) VALUES ('agent-a', 980, 1000, '2026-03-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        update_condition(
            &pool,
            "host.memory_pressure",
            RuleCondition::with_threshold(1, 120, 90.0),
        )
        .await;

        let t0 = base_time();
        let changes = report_eval(&pool, "agent-a", &["node-a".to_owned()], t0).await;
        assert!(changes > 0);
        let changes = report_eval(
            &pool,
            "agent-a",
            &["node-a".to_owned()],
            t0 + time::Duration::seconds(5),
        )
        .await;
        assert!(changes > 0);

        let rpc_open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM alert_incidents WHERE rule_key = 'node.rpc_unreachable' AND state = 'open'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rpc_open, 1);
        let memory_open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM alert_incidents WHERE rule_key = 'host.memory_pressure' AND state = 'open'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(memory_open, 1);
    }
}
