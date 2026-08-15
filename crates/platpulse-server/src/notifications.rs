//! Notification Events and Delivery (issue #49, design §17.4/§17.5).
//!
//! Notification Events are durable business records of an Incident
//! transition (or an Owner test action); Notification Deliveries are the
//! per-channel/destination attempts of one Event. Delivery is
//! at-least-once, never exactly-once: an Event and its Outbox rows are
//! written in the same transaction as the Incident transition, automatic
//! retries are bounded with exponential backoff and provider Retry-After,
//! exhausted Deliveries reach DeadLetter, and a manual Owner retry
//! re-arms the same Delivery row -- it never creates a duplicate Event or
//! business transition. Provider tokens live in dedicated secret files and
//! never enter the database, DTOs, logs, or Audit rows; destinations and
//! provider results are redacted by construction.

use std::path::Path;
use std::time::Duration;

use crate::alerts::{SubjectKind, suppressions_for_subject};
use crate::auth::{format_rfc3339, now_utc};
use crate::config::{NotificationChannels, TelegramChannel};
use async_trait::async_trait;
use serde::Serialize;
use sqlx::FromRow;
use sqlx::SqliteConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;

/// Delivery worker poll cadence (design §17.4: the worker survives
/// restarts; a crash mid-send is repaired on the next pass).
pub const DELIVERY_INTERVAL: Duration = Duration::from_secs(5);

/// Upper bound for one exponential backoff step.
const MAX_BACKOFF_SECS: i64 = 3600;

/// Per-pass batch bound so one pass never monopolizes the single
/// SQLite connection.
const BATCH_LIMIT: i64 = 20;

pub const DELIVERY_STATES: &[&str] = &[
    "pending",
    "in_flight",
    "retry_scheduled",
    "succeeded",
    "failed",
    "dead_letter",
    "suppressed",
];

/// One Notification Delivery row (redacted by construction: `destination`
/// is a masked summary and `last_result` is a fixed provider vocabulary,
/// never raw provider output).
#[derive(Debug, Clone, Serialize, ToSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRow {
    pub delivery_id: String,
    pub event_id: String,
    pub channel_kind: String,
    pub destination: String,
    pub state: String,
    pub attempt_count: i64,
    pub next_attempt_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_result: Option<String>,
    pub last_error_kind: Option<String>,
    pub retry_after_seconds: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// One recorded provider attempt (redacted provider result).
#[derive(Debug, Clone, Serialize, ToSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRow {
    pub attempt_id: String,
    pub delivery_id: String,
    pub attempt_number: i64,
    pub attempted_at: String,
    pub outcome: String,
    pub provider_result: String,
    pub error_kind: Option<String>,
    pub duration_ms: Option<i64>,
    pub retry_after_seconds: Option<i64>,
}

/// One Notification Event.
#[derive(Debug, Clone, Serialize, ToSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub event_id: String,
    pub event_kind: String,
    pub incident_id: Option<String>,
    pub rule_key: Option<String>,
    pub subject_kind: Option<String>,
    pub subject_key: Option<String>,
    pub severity: String,
    pub summary: String,
    pub created_at: String,
}

/// The provider outcome of one send. The error vocabulary is fixed so
/// provider output is never persisted raw: `provider_result` strings are
/// derived from these variants only.
#[derive(Debug)]
pub enum SendError {
    /// The provider returned a business error (Telegram `error_code`).
    /// `retry_after` carries the provider's 429 Retry-After when present.
    Api { code: i64, retry_after: Option<i64> },
    /// Transport-level failure (DNS, connect, TLS, reset).
    Network,
    /// The provider did not answer within the client timeout.
    Timeout,
    /// The channel cannot be used right now (missing token file, disabled
    /// channel). Retrying automatically cannot help.
    Config { reason: String },
}

impl SendError {
    pub fn error_kind(&self) -> &'static str {
        match self {
            SendError::Api { .. } => "telegram_api",
            SendError::Network => "network",
            SendError::Timeout => "timeout",
            SendError::Config { .. } => "config",
        }
    }

    /// Redacted, fixed-vocabulary provider result for storage and DTOs.
    pub fn provider_result(&self) -> String {
        match self {
            SendError::Api { code, .. } => format!("telegram_api_error {code}"),
            SendError::Network => "telegram_network_error".to_owned(),
            SendError::Timeout => "telegram_timeout".to_owned(),
            SendError::Config { reason } => format!("channel_config_error: {reason}"),
        }
    }

    pub fn retry_after_seconds(&self) -> Option<i64> {
        match self {
            SendError::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// Deterministic delivery double used in development mode (e2e and local
/// development): every send fails with a fixed API error, so delivery
/// state machines, bounded retry, and DeadLetter behavior are fully
/// observable without touching a real provider or the network. Production
/// always uses `TelegramProvider`; tests inject their own doubles.
pub struct DevNullProvider;

#[async_trait]
impl DeliveryProvider for DevNullProvider {
    async fn send(&self, _channel: &TelegramChannel, _text: &str) -> Result<(), SendError> {
        Err(SendError::Api {
            code: 401,
            retry_after: None,
        })
    }
}

/// The real external adapter seam for notification channels (design §20).
/// The Server's only production implementation is Telegram; tests inject
/// deterministic doubles through the same seam.
#[async_trait]
pub trait DeliveryProvider: Send + Sync {
    async fn send(&self, channel: &TelegramChannel, text: &str) -> Result<(), SendError>;
}

/// Parse a Telegram Bot API response into the fixed provider vocabulary.
/// Telegram reports failures as `{ok:false, error_code, description}` —
/// normally on a non-2xx HTTP status but occasionally on 200 — and the
/// 429 Retry-After lives under `parameters.retry_after`. Only the code and
/// the explicit Retry-After survive; descriptions are never stored or
/// logged.
fn parse_telegram_response(status: u16, body: &serde_json::Value) -> Result<(), SendError> {
    let ok = body
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let code = body
        .get("error_code")
        .and_then(|value| value.as_i64())
        .unwrap_or(i64::from(status));
    let retry_after = body
        .get("parameters")
        .and_then(|parameters| parameters.get("retry_after"))
        .and_then(|value| value.as_i64())
        .or_else(|| body.get("retry_after").and_then(|value| value.as_i64()));
    if ok && (200..300).contains(&status) {
        return Ok(());
    }
    Err(SendError::Api { code, retry_after })
}

/// Telegram Bot API sender (the approved Phase 2 delivery path). The bot
/// token is read from the configured secret file for every send, so
/// rotation takes effect without a restart and the token never lives in
/// the process beyond one send.
pub struct TelegramProvider {
    http: reqwest::Client,
}

impl Default for TelegramProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TelegramProvider {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client builds");
        Self { http }
    }

    async fn send_inner(&self, token: &str, chat_id: &str, text: &str) -> Result<(), SendError> {
        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        // The URL embeds the token; it is never logged. Errors below are
        // mapped onto the fixed vocabulary, never the raw body.
        let response = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": text,
                "disable_web_page_preview": true,
            }))
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    SendError::Timeout
                } else {
                    SendError::Network
                }
            })?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .unwrap_or_else(|_| serde_json::json!({}));
        parse_telegram_response(status.as_u16(), &body)
    }
}

#[async_trait]
impl DeliveryProvider for TelegramProvider {
    async fn send(&self, channel: &TelegramChannel, text: &str) -> Result<(), SendError> {
        let token = std::fs::read_to_string(&channel.token_file)
            .map_err(|_| SendError::Config {
                reason: "token file unreadable".to_owned(),
            })?
            .trim()
            .to_owned();
        if token.is_empty() {
            return Err(SendError::Config {
                reason: "token file empty".to_owned(),
            });
        }
        self.send_inner(&token, &channel.chat_id, text).await
    }
}

/// Mask a destination for storage and display: only the last four
/// characters survive (`****1234`); shorter values are fully masked. The
/// full destination never enters DTOs, Audit bodies, or logs.
pub fn redact_destination(chat_id: &str) -> String {
    if chat_id.len() <= 4 {
        "****".to_owned()
    } else {
        format!("****{}", &chat_id[chat_id.len() - 4..])
    }
}

/// Redacted provider reference for a configured channel: only the secret
/// file base name is exposed, never the directory or the token.
pub fn provider_reference(token_file: &Path) -> String {
    token_file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("secret")
        .to_owned()
}

/// Compose the message text sent to a channel for one Event. Content is
/// Server-owned and bounded (Telegram's limit is 4096 characters).
pub fn message_text(event: &EventRow) -> String {
    let tag = match event.event_kind.as_str() {
        "test" => "TEST",
        _ => "ALERT",
    };
    match (&event.rule_key, &event.subject_kind, &event.subject_key) {
        (Some(rule), Some(kind), Some(subject)) => format!(
            "[PlatPulse {tag}] {severity} {rule}\n{kind} {subject}\n{summary}",
            severity = event.severity.to_uppercase(),
            summary = event.summary,
        ),
        _ => format!("[PlatPulse {tag}] {summary}", summary = event.summary),
    }
}

// ---------------------------------------------------------------------------
// Event + Outbox creation (design §17.4: same transaction as the Incident
// transition)
// ---------------------------------------------------------------------------

/// One Notification Event to record (design §17.4). Test events carry no
/// Incident/subject context.
pub struct NotificationEventInput<'a> {
    pub kind: &'a str,
    pub incident_id: Option<&'a str>,
    pub rule_key: Option<&'a str>,
    pub subject: Option<(SubjectKind, &'a str)>,
    pub severity: &'a str,
    pub summary: &'a str,
}

/// Create one durable Notification Event plus one Delivery per configured
/// channel inside the caller's transaction. Active Silence/Maintenance
/// policies suppress Delivery at creation time (design §17.5): suppressed
/// Deliveries are never sent and are not retryable. Returns the Event id.
pub async fn record_notification_event(
    executor: &mut SqliteConnection,
    input: NotificationEventInput<'_>,
    channels: &NotificationChannels,
    now: OffsetDateTime,
) -> Result<String, sqlx::Error> {
    let event_id = uuid::Uuid::new_v4().to_string();
    let created_at = format_rfc3339(now);
    sqlx::query(
        "INSERT INTO notification_events (event_id, event_kind, incident_id, rule_key, subject_kind, subject_key, severity, summary, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&event_id)
    .bind(input.kind)
    .bind(input.incident_id)
    .bind(input.rule_key)
    .bind(input.subject.as_ref().map(|(kind, _)| kind.as_str()))
    .bind(input.subject.as_ref().map(|(_, key)| *key))
    .bind(input.severity)
    .bind(input.summary)
    .bind(&created_at)
    .execute(&mut *executor)
    .await?;

    let suppressed = match (input.kind, input.rule_key, input.subject) {
        ("incident", Some(rule_key), Some((subject_kind, subject_key))) => {
            let matches =
                suppressions_for_subject(executor, rule_key, subject_kind, subject_key, now)
                    .await?;
            matches
                .first()
                .map(|matched| format!("suppressed_by_{}:{}", matched.kind, matched.id))
        }
        _ => None,
    };

    if let Some(telegram) = channels.telegram() {
        let delivery_id = uuid::Uuid::new_v4().to_string();
        let (state, last_result) = match &suppressed {
            Some(reason) => ("suppressed", Some(reason.as_str())),
            None => ("pending", None),
        };
        sqlx::query(
            "INSERT INTO notification_deliveries (delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at) VALUES (?, ?, 'telegram', ?, ?, 0, NULL, NULL, ?, NULL, NULL, ?, ?)",
        )
        .bind(&delivery_id)
        .bind(&event_id)
        .bind(redact_destination(&telegram.chat_id))
        .bind(state)
        .bind(last_result)
        .bind(&created_at)
        .bind(&created_at)
        .execute(&mut *executor)
        .await?;
    }
    Ok(event_id)
}

/// A test Notification Event (event_kind `test`): created by the Owner
/// action, never derived from an Incident transition, and always audited
/// by the caller. Test Deliveries are never suppressed.
pub async fn record_test_event(
    executor: &mut SqliteConnection,
    severity: &str,
    summary: &str,
    channels: &NotificationChannels,
    now: OffsetDateTime,
) -> Result<String, sqlx::Error> {
    record_notification_event(
        executor,
        NotificationEventInput {
            kind: "test",
            incident_id: None,
            rule_key: None,
            subject: None,
            severity,
            summary,
        },
        channels,
        now,
    )
    .await
}

// ---------------------------------------------------------------------------
// Delivery state machine
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum RetryError {
    NotFound,
    AlreadyQueued,
    NotRetryable { state: String },
}

/// Manual Owner retry: re-arms an existing Delivery for the worker. It
/// never creates a new Event, Incident, or Delivery row; the next worker
/// pass records a fresh attempt on the same row. Duplicate parallel
/// retries are refused by the Server (pending/in_flight rows are already
/// queued). Suppressed and succeeded Deliveries are not retryable.
pub async fn rearm_delivery(
    executor: &mut SqliteConnection,
    delivery_id: &str,
    now: OffsetDateTime,
) -> Result<DeliveryRow, RetryError> {
    let current: Option<String> =
        sqlx::query_scalar("SELECT state FROM notification_deliveries WHERE delivery_id = ?")
            .bind(delivery_id)
            .fetch_optional(&mut *executor)
            .await
            .map_err(|_| RetryError::NotFound)?;
    let Some(state) = current else {
        return Err(RetryError::NotFound);
    };
    if matches!(state.as_str(), "pending" | "in_flight") {
        return Err(RetryError::AlreadyQueued);
    }
    if !matches!(state.as_str(), "retry_scheduled" | "failed" | "dead_letter") {
        return Err(RetryError::NotRetryable { state });
    }
    let updated_at = format_rfc3339(now);
    sqlx::query(
        "UPDATE notification_deliveries SET state = 'pending', next_attempt_at = NULL, updated_at = ? WHERE delivery_id = ? AND state = ?",
    )
    .bind(&updated_at)
    .bind(delivery_id)
    .bind(&state)
    .execute(&mut *executor)
    .await
    .map_err(|_| RetryError::NotFound)?;
    load_delivery(executor, delivery_id)
        .await
        .map_err(|_| RetryError::NotFound)?
        .ok_or(RetryError::NotFound)
}

pub async fn load_delivery(
    executor: &mut SqliteConnection,
    delivery_id: &str,
) -> Result<Option<DeliveryRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, DeliveryRow>(
        "SELECT delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at FROM notification_deliveries WHERE delivery_id = ?",
    )
    .bind(delivery_id)
    .fetch_optional(&mut *executor)
    .await?;
    Ok(row)
}

pub async fn load_event(
    executor: &mut SqliteConnection,
    event_id: &str,
) -> Result<Option<EventRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, EventRow>(
        "SELECT event_id, event_kind, incident_id, rule_key, subject_kind, subject_key, severity, summary, created_at FROM notification_events WHERE event_id = ?",
    )
    .bind(event_id)
    .fetch_optional(&mut *executor)
    .await?;
    Ok(row)
}

pub async fn deliveries_for_event(
    executor: &mut SqliteConnection,
    event_id: &str,
) -> Result<Vec<DeliveryRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DeliveryRow>(
        "SELECT delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at FROM notification_deliveries WHERE event_id = ? ORDER BY created_at",
    )
    .bind(event_id)
    .fetch_all(&mut *executor)
    .await?;
    Ok(rows)
}

pub async fn attempts_for_delivery(
    executor: &mut SqliteConnection,
    delivery_id: &str,
) -> Result<Vec<AttemptRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, AttemptRow>(
        "SELECT attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds FROM delivery_attempts WHERE delivery_id = ? ORDER BY attempt_number",
    )
    .bind(delivery_id)
    .fetch_all(&mut *executor)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// One pass of the delivery worker:
/// 1. repair stale `in_flight` rows (crash recovery, at-least-once);
/// 2. claim due Deliveries and send them through the provider;
/// 3. record one attempt row and the resulting state per Delivery;
/// 4. publish an Admin invalidation when anything changed.
///
/// The single connection pool serializes all writes; claims are atomic
/// `UPDATE ... WHERE state = 'pending'` so a parallel manual test send
/// can never double-send. Returns the number of processed Deliveries.
pub async fn process_due_deliveries(
    state: &crate::http::AppState,
    provider: &dyn DeliveryProvider,
) -> Result<usize, sqlx::Error> {
    // Like the evaluation sweep, one pass is a bounded critical unit: the
    // ingestion guard makes graceful shutdown drain the current pass
    // (including an in-flight provider send) before the WAL checkpoint and
    // database close (design §17.4: sends complete or return to
    // RetryScheduled; a Delivery left in_flight is repaired on next boot).
    let Some(_guard) = state.ingestion_guard() else {
        return Ok(0);
    };
    let now = now_utc();
    let mut processed = 0usize;

    // Crash recovery: a Delivery left in_flight by a dead process was
    // claimed but never recorded; re-queue it (at-least-once allows the
    // duplicate the provider may have already seen).
    {
        let mut tx = state.db().pool().begin().await?;
        sqlx::query(
            "UPDATE notification_deliveries SET state = 'pending', next_attempt_at = NULL, updated_at = ? WHERE state = 'in_flight'",
        )
        .bind(format_rfc3339(now))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    let now_text = format_rfc3339(now);
    let due: Vec<DeliveryRow> = {
        let mut conn = state.db().pool().acquire().await?;
        sqlx::query_as::<_, DeliveryRow>(
            "SELECT delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at FROM notification_deliveries WHERE state IN ('pending', 'retry_scheduled') AND (next_attempt_at IS NULL OR next_attempt_at <= ?) ORDER BY created_at LIMIT ?",
        )
        .bind(&now_text)
        .bind(BATCH_LIMIT)
        .fetch_all(&mut *conn)
        .await?
    };

    for delivery in due {
        // Claim atomically: a parallel retry/test path or a previous pass
        // may already hold this row.
        let claimed = {
            let mut tx = state.db().pool().begin().await?;
            let result = sqlx::query(
                "UPDATE notification_deliveries SET state = 'in_flight', updated_at = ? WHERE delivery_id = ? AND state IN ('pending', 'retry_scheduled')",
            )
            .bind(&now_text)
            .bind(&delivery.delivery_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            result.rows_affected() > 0
        };
        if !claimed {
            continue;
        }

        let event = {
            let mut conn = state.db().pool().acquire().await?;
            load_event(&mut conn, &delivery.event_id).await?
        };
        let channel = state.channels().telegram();
        let send_result = match (event.as_ref(), channel) {
            (Some(event), Some(channel)) if channel.enabled => {
                provider.send(channel, &message_text(event)).await
            }
            (None, _) => Err(SendError::Config {
                reason: "event missing".to_owned(),
            }),
            (Some(_), Some(_)) => Err(SendError::Config {
                reason: "channel disabled".to_owned(),
            }),
            (Some(_), None) => Err(SendError::Config {
                reason: "channel not configured".to_owned(),
            }),
        };

        let attempted_at = format_rfc3339(now_utc());
        let attempt_number = delivery.attempt_count + 1;
        let max_attempts = channel
            .map(|channel| channel.max_attempts as i64)
            .unwrap_or(1);
        let backoff_base = channel
            .map(|channel| channel.retry_base_seconds as i64)
            .unwrap_or(60);
        // Config-level failures are terminal and never consume an attempt:
        // retrying cannot help until deployment config changes, and the
        // attempt history stays a record of real provider sends.
        let is_config_error = matches!(&send_result, Err(SendError::Config { .. }));
        // Redacted, fixed-vocabulary result shared by the state arms.
        let (result_kind, provider_result, error_kind, retry_after) = match &send_result {
            Ok(()) => ("succeeded", "ok".to_owned(), None, None),
            Err(error) => (
                "failed",
                error.provider_result(),
                Some(error.error_kind().to_owned()),
                error.retry_after_seconds(),
            ),
        };
        let (next_state, next_attempt_at, last_result, last_error_kind, retry_after) =
            if is_config_error {
                // Config-level failures are terminal and never consume an
                // attempt: retrying cannot help until deployment config
                // changes, and the attempt history stays a record of real
                // provider sends.
                (
                    Some("failed"),
                    None,
                    Some(provider_result),
                    Some("config".to_owned()),
                    None,
                )
            } else if result_kind == "succeeded" {
                (Some("succeeded"), None, Some(provider_result), None, None)
            } else if attempt_number >= max_attempts {
                (
                    Some("dead_letter"),
                    None,
                    Some(provider_result),
                    error_kind,
                    retry_after,
                )
            } else {
                let backoff =
                    (backoff_base * 2i64.pow((attempt_number - 1) as u32)).min(MAX_BACKOFF_SECS);
                let wait = retry_after.map(|secs| secs.max(backoff)).unwrap_or(backoff);
                let next = now_utc() + time::Duration::seconds(wait);
                (
                    Some("retry_scheduled"),
                    Some(format_rfc3339(next)),
                    Some(provider_result),
                    error_kind,
                    retry_after,
                )
            };

        let mut tx = state.db().pool().begin().await?;
        if !is_config_error {
            if let Err(error) = &send_result {
                sqlx::query(
                    "INSERT INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES (?, ?, ?, ?, 'failed', ?, ?, NULL, ?)",
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&delivery.delivery_id)
                .bind(attempt_number)
                .bind(&attempted_at)
                .bind(error.provider_result())
                .bind(error.error_kind())
                .bind(error.retry_after_seconds())
                .execute(&mut *tx)
                .await?;
            } else {
                sqlx::query(
                    "INSERT INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES (?, ?, ?, ?, 'succeeded', 'ok', NULL, NULL, NULL)",
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&delivery.delivery_id)
                .bind(attempt_number)
                .bind(&attempted_at)
                .execute(&mut *tx)
                .await?;
            }
        }
        let updated_attempt_count = if is_config_error {
            delivery.attempt_count
        } else {
            attempt_number
        };
        sqlx::query(
            "UPDATE notification_deliveries SET state = ?, attempt_count = ?, next_attempt_at = ?, last_attempt_at = ?, last_result = ?, last_error_kind = ?, retry_after_seconds = ?, updated_at = ? WHERE delivery_id = ?",
        )
        .bind(next_state)
        .bind(updated_attempt_count)
        .bind(next_attempt_at)
        .bind(&attempted_at)
        .bind(last_result)
        .bind(last_error_kind)
        .bind(retry_after)
        .bind(&attempted_at)
        .bind(&delivery.delivery_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        processed += 1;
    }

    if processed > 0 {
        state
            .admin_realtime()
            .publish("notifications", None::<String>, processed as u64);
    }
    Ok(processed)
}

/// Synchronous send path used by the Owner test action: creates the test
/// Event + Delivery, then immediately claims and sends it so the response
/// carries the resulting state. Returns `(event_id, delivery)`.
pub async fn send_test_delivery(
    state: &crate::http::AppState,
    provider: &dyn DeliveryProvider,
    severity: &str,
    summary: &str,
) -> Result<(String, DeliveryRow), TestSendError> {
    let now = now_utc();
    let Some(telegram) = state.channels().telegram() else {
        return Err(TestSendError::NotConfigured);
    };
    if !telegram.enabled {
        return Err(TestSendError::Disabled);
    }
    let mut tx = state
        .db()
        .pool()
        .begin()
        .await
        .map_err(|_| TestSendError::Unavailable)?;
    let event_id = record_test_event(&mut tx, severity, summary, state.channels(), now)
        .await
        .map_err(|_| TestSendError::Unavailable)?;
    let delivery_id: String =
        sqlx::query_scalar("SELECT delivery_id FROM notification_deliveries WHERE event_id = ?")
            .bind(&event_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| TestSendError::Unavailable)?;
    tx.commit().await.map_err(|_| TestSendError::Unavailable)?;

    // Claim and send synchronously. If the worker already claimed the
    // Delivery, return it as-is (the worker finishes it; no double send).
    let claimed = {
        let mut conn = state
            .db()
            .pool()
            .acquire()
            .await
            .map_err(|_| TestSendError::Unavailable)?;
        let result = sqlx::query(
            "UPDATE notification_deliveries SET state = 'in_flight', updated_at = ? WHERE delivery_id = ? AND state = 'pending'",
        )
        .bind(format_rfc3339(now_utc()))
        .bind(&delivery_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| TestSendError::Unavailable)?;
        result.rows_affected() > 0
    };
    if claimed {
        let event = {
            let mut conn = state
                .db()
                .pool()
                .acquire()
                .await
                .map_err(|_| TestSendError::Unavailable)?;
            load_event(&mut conn, &event_id)
                .await
                .map_err(|_| TestSendError::Unavailable)?
        }
        .expect("test event exists");
        let send_result = provider.send(telegram, &message_text(&event)).await;
        let attempted_at = format_rfc3339(now_utc());
        let mut tx = state
            .db()
            .pool()
            .begin()
            .await
            .map_err(|_| TestSendError::Unavailable)?;
        match &send_result {
            Ok(()) => {
                sqlx::query(
                    "INSERT INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES (?, ?, 1, ?, 'succeeded', 'ok', NULL, NULL, NULL)",
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&delivery_id)
                .bind(&attempted_at)
                .execute(&mut *tx)
                .await
                .map_err(|_| TestSendError::Unavailable)?;
                sqlx::query(
                    "UPDATE notification_deliveries SET state = 'succeeded', attempt_count = 1, next_attempt_at = NULL, last_attempt_at = ?, last_result = 'ok', last_error_kind = NULL, retry_after_seconds = NULL, updated_at = ? WHERE delivery_id = ?",
                )
                .bind(&attempted_at)
                .bind(&attempted_at)
                .bind(&delivery_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| TestSendError::Unavailable)?;
            }
            Err(error) => {
                sqlx::query(
                    "INSERT INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES (?, ?, 1, ?, 'failed', ?, ?, NULL, ?)",
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&delivery_id)
                .bind(&attempted_at)
                .bind(error.provider_result())
                .bind(error.error_kind())
                .bind(error.retry_after_seconds())
                .execute(&mut *tx)
                .await
                .map_err(|_| TestSendError::Unavailable)?;
                sqlx::query(
                    "UPDATE notification_deliveries SET state = 'failed', attempt_count = 1, next_attempt_at = NULL, last_attempt_at = ?, last_result = ?, last_error_kind = ?, retry_after_seconds = ?, updated_at = ? WHERE delivery_id = ?",
                )
                .bind(&attempted_at)
                .bind(error.provider_result())
                .bind(error.error_kind())
                .bind(error.retry_after_seconds())
                .bind(&attempted_at)
                .bind(&delivery_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| TestSendError::Unavailable)?;
            }
        }
        tx.commit().await.map_err(|_| TestSendError::Unavailable)?;
        state
            .admin_realtime()
            .publish("notifications", None::<String>, 1);
    }
    let mut conn = state
        .db()
        .pool()
        .acquire()
        .await
        .map_err(|_| TestSendError::Unavailable)?;
    let delivery = load_delivery(&mut conn, &delivery_id)
        .await
        .map_err(|_| TestSendError::Unavailable)?
        .expect("delivery exists");
    Ok((event_id, delivery))
}

#[derive(Debug)]
pub enum TestSendError {
    NotConfigured,
    Disabled,
    Unavailable,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::format_rfc3339;
    use crate::config::TelegramChannel;
    use sqlx::SqlitePool;
    use tempfile::tempdir;
    use time::OffsetDateTime;
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

    /// `notification_events.incident_id` references `alert_incidents`;
    /// tests that attach an Event to an Incident seed the Incident first.
    async fn seed_incident(pool: &SqlitePool) {
        sqlx::query("INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('inc-1', 'node.rpc_unreachable', 1, 'node', 'node-a', 'critical', 'open', '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z', '{}') ON CONFLICT(incident_id) DO NOTHING")
            .execute(pool)
            .await
            .unwrap();
    }

    fn telegram_channel(enabled: bool) -> TelegramChannel {
        TelegramChannel {
            enabled,
            token_file: Path::new("/etc/platpulse/secrets/telegram-token").to_owned(),
            chat_id: "123456789".to_owned(),
            max_attempts: 3,
            retry_base_seconds: 60,
        }
    }

    fn channels(telegram: Option<TelegramChannel>) -> NotificationChannels {
        NotificationChannels { telegram }
    }

    #[tokio::test]
    async fn event_and_deliveries_are_created_in_one_record() {
        let (_dir, pool) = test_pool().await;
        seed_incident(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        let event_id = record_notification_event(
            &mut conn,
            NotificationEventInput {
                kind: "incident",
                incident_id: Some("inc-1"),
                rule_key: Some("node.rpc_unreachable"),
                subject: Some((SubjectKind::Node, "node-a")),
                severity: "critical",
                summary: "Incident opened: node.rpc_unreachable on node node-a",
            },
            &channels(Some(telegram_channel(true))),
            base_time(),
        )
        .await
        .unwrap();
        let event = load_event(&mut conn, &event_id).await.unwrap().unwrap();
        assert_eq!(event.event_kind, "incident");
        assert_eq!(event.incident_id.as_deref(), Some("inc-1"));
        let deliveries = deliveries_for_event(&mut conn, &event_id).await.unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].state, "pending");
        assert_eq!(deliveries[0].channel_kind, "telegram");
        assert_eq!(deliveries[0].destination, "****6789");
        assert_eq!(deliveries[0].attempt_count, 0);
        // The same Event never produces a second Delivery row.
        let second = record_notification_event(
            &mut conn,
            NotificationEventInput {
                kind: "incident",
                incident_id: Some("inc-1"),
                rule_key: Some("node.rpc_unreachable"),
                subject: Some((SubjectKind::Node, "node-a")),
                severity: "critical",
                summary: "Incident opened: node.rpc_unreachable on node node-a",
            },
            &channels(Some(telegram_channel(true))),
            base_time(),
        )
        .await
        .unwrap();
        assert_ne!(second, event_id);
    }

    #[tokio::test]
    async fn unconfigured_channels_create_events_without_deliveries() {
        let (_dir, pool) = test_pool().await;
        seed_incident(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        let event_id = record_notification_event(
            &mut conn,
            NotificationEventInput {
                kind: "incident",
                incident_id: Some("inc-1"),
                rule_key: Some("node.rpc_unreachable"),
                subject: Some((SubjectKind::Node, "node-a")),
                severity: "warning",
                summary: "Incident opened",
            },
            &channels(None),
            base_time(),
        )
        .await
        .unwrap();
        let deliveries = deliveries_for_event(&mut conn, &event_id).await.unwrap();
        assert!(deliveries.is_empty());
    }

    #[tokio::test]
    async fn active_silence_suppresses_the_delivery_but_keeps_the_event() {
        let (_dir, pool) = test_pool().await;
        seed_incident(&pool).await;
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO silences (silence_id, matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at) VALUES ('sil-1', 'node', 'node-a', 'on purpose', '2026-03-01T00:00:00Z', '2026-03-02T00:00:00Z', 'owner', '2026-03-01T00:00:00Z')")
            .execute(&pool).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let event_id = record_notification_event(
            &mut conn,
            NotificationEventInput {
                kind: "incident",
                incident_id: Some("inc-1"),
                rule_key: Some("node.rpc_unreachable"),
                subject: Some((SubjectKind::Node, "node-a")),
                severity: "warning",
                summary: "Incident opened",
            },
            &channels(Some(telegram_channel(true))),
            base_time(),
        )
        .await
        .unwrap();
        let deliveries = deliveries_for_event(&mut conn, &event_id).await.unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].state, "suppressed");
        assert_eq!(
            deliveries[0].last_result.as_deref(),
            Some("suppressed_by_silence:sil-1")
        );
    }

    #[tokio::test]
    async fn manual_retry_rearms_only_retryable_states() {
        let (_dir, pool) = test_pool().await;
        seed_incident(&pool).await;
        let now = base_time();
        let mut conn = pool.acquire().await.unwrap();
        let event_id = record_notification_event(
            &mut conn,
            NotificationEventInput {
                kind: "incident",
                incident_id: None,
                rule_key: None,
                subject: None,
                severity: "info",
                summary: "summary",
            },
            &channels(Some(telegram_channel(true))),
            now,
        )
        .await
        .unwrap();
        let delivery_id = deliveries_for_event(&mut conn, &event_id).await.unwrap()[0]
            .delivery_id
            .clone();
        let retry = rearm_delivery(&mut conn, &delivery_id, now)
            .await
            .unwrap_err();
        assert!(matches!(retry, RetryError::AlreadyQueued));

        for (state, expected) in [
            (
                "succeeded",
                RetryError::NotRetryable {
                    state: "succeeded".into(),
                },
            ),
            (
                "suppressed",
                RetryError::NotRetryable {
                    state: "suppressed".into(),
                },
            ),
            ("in_flight", RetryError::AlreadyQueued),
            ("pending", RetryError::AlreadyQueued),
        ] {
            sqlx::query("UPDATE notification_deliveries SET state = ? WHERE delivery_id = ?")
                .bind(state)
                .bind(&delivery_id)
                .execute(&mut *conn)
                .await
                .unwrap();
            let outcome = rearm_delivery(&mut conn, &delivery_id, now)
                .await
                .unwrap_err();
            assert!(
                matches!(outcome, RetryError::NotRetryable { .. })
                    == matches!(expected, RetryError::NotRetryable { .. })
            );
            assert!(
                matches!(outcome, RetryError::AlreadyQueued)
                    == matches!(expected, RetryError::AlreadyQueued)
            );
        }

        for state in ["retry_scheduled", "failed", "dead_letter"] {
            sqlx::query("UPDATE notification_deliveries SET state = ? WHERE delivery_id = ?")
                .bind(state)
                .bind(&delivery_id)
                .execute(&mut *conn)
                .await
                .unwrap();
            let delivery = rearm_delivery(&mut conn, &delivery_id, now).await.unwrap();
            assert_eq!(delivery.state, "pending");
            assert_eq!(delivery.next_attempt_at, None);
            // The same Delivery row is reused: no duplicate Event/Delivery.
            assert_eq!(delivery.delivery_id, delivery_id);
        }
    }

    #[tokio::test]
    async fn redaction_masks_short_and_long_destinations() {
        assert_eq!(redact_destination("123456789"), "****6789");
        assert_eq!(redact_destination("12"), "****");
        assert_eq!(
            provider_reference(Path::new("/etc/platpulse/secrets/telegram-token")),
            "telegram-token"
        );
        assert_eq!(
            provider_reference(Path::new("/var/lib/platpulse/pepper")),
            "pepper"
        );
    }

    #[test]
    fn telegram_response_parsing_honors_ok_flag_and_parameters_retry_after() {
        // Success: 2xx plus `ok: true`.
        assert!(matches!(
            parse_telegram_response(200, &serde_json::json!({"ok": true})),
            Ok(())
        ));
        // A JSON-level failure riding on a 2xx status must not look like
        // a success (Telegram occasionally reports failures this way).
        assert!(matches!(
            parse_telegram_response(200, &serde_json::json!({"ok": false, "error_code": 400})),
            Err(SendError::Api {
                code: 400,
                retry_after: None
            })
        ));
        // The 429 Retry-After lives under `parameters` in the Telegram
        // body; only the code and the explicit Retry-After survive.
        assert!(matches!(
            parse_telegram_response(
                429,
                &serde_json::json!({
                    "ok": false,
                    "error_code": 429,
                    "description": "Too Many Requests: retry after 7",
                    "parameters": {"retry_after": 7}
                })
            ),
            Err(SendError::Api {
                code: 429,
                retry_after: Some(7)
            })
        ));
        // Defensive fallback: a root-level retry_after is honored too.
        assert!(matches!(
            parse_telegram_response(
                429,
                &serde_json::json!({"ok": false, "error_code": 429, "retry_after": 9})
            ),
            Err(SendError::Api {
                code: 429,
                retry_after: Some(9)
            })
        ));
        // Non-2xx with an unparseable body still carries the HTTP code.
        assert!(matches!(
            parse_telegram_response(500, &serde_json::json!({})),
            Err(SendError::Api {
                code: 500,
                retry_after: None
            })
        ));
        // Descriptions never leak into the fixed vocabulary.
        let error = parse_telegram_response(
            400,
            &serde_json::json!({"ok": false, "error_code": 400, "description": "chat not found"}),
        )
        .unwrap_err();
        assert_eq!(error.provider_result(), "telegram_api_error 400");
        assert!(!error.provider_result().contains("chat"));
    }

    #[tokio::test]
    async fn message_text_contains_only_server_owned_fields() {
        let event = EventRow {
            event_id: "e".into(),
            event_kind: "incident".into(),
            incident_id: Some("inc-1".into()),
            rule_key: Some("node.rpc_unreachable".into()),
            subject_kind: Some("node".into()),
            subject_key: Some("node-a".into()),
            severity: "critical".into(),
            summary: "Incident opened: node.rpc_unreachable on node node-a".into(),
            created_at: format_rfc3339(base_time()),
        };
        let text = message_text(&event);
        assert!(text.contains("node.rpc_unreachable"));
        assert!(text.contains("CRITICAL"));
        assert!(!text.contains("inc-1"));
        let test = EventRow {
            event_kind: "test".into(),
            ..event
        };
        assert!(message_text(&test).contains("TEST"));
    }
}
