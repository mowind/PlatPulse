//! Capacity visibility and low-space protection for the Server database
//! filesystem (issue #212, design §11.4, parent #202 stories 43-46).
//!
//! The Server writes two very different kinds of data:
//!
//! * The core ingestion transaction: the current projection, the Report
//!   allocation and its receipt. It must either commit or fail loudly
//!   (report_ingestion.rs), and nothing here weakens that obligation.
//! * Optional generic metric history (save_node_metric / save_host_metric). It
//!   is bounded per series and is an extra, not a promise the rest of the
//!   Server depends on.
//!
//! When the filesystem holding the Server database runs out of room, the
//! second kind is what gives way. This module measures the filesystem, decides
//! with hysteresis whether protection is active, keeps that answer durable
//! (capacity_protection_intervals), and hands every ingestion a gate saying
//! "record optional history" or "skip it and record the gap against this
//! interval". Protection never deletes or re-encodes retained history, never
//! lowers precision, and never turns a failed core write into a success.
//!
//! Thresholds deliberately have no built-in default: a fabricated GiB figure
//! would be an unmeasured promise about a deployment nobody looked at. An
//! enabled policy must declare its own thresholds, and both the values in force
//! and the measurement that justified entering and leaving protection are
//! recorded in the interval row.
//!
//! Measurement failures fail open. A failed statvfs cannot distinguish "no
//! room" from "temporarily unreadable", so the Server keeps its previous
//! protection state and records the error for the Operator instead of
//! inventing a gap.

use std::{
    path::{Path, PathBuf},
    sync::RwLock,
    time::Duration,
};

use serde::Serialize;
use sqlx::{Sqlite, SqlitePool, Transaction};
use thiserror::Error;

use crate::auth::{format_rfc3339, now_utc};

/// Sampling cadence used when an enabled policy does not declare one.
pub const DEFAULT_SAMPLE_INTERVAL_SECONDS: u64 = 60;
/// Fastest supported sampling cadence.
pub const MIN_SAMPLE_INTERVAL_SECONDS: u64 = 5;
/// Slowest supported sampling cadence.
pub const MAX_SAMPLE_INTERVAL_SECONDS: u64 = 24 * 60 * 60;

/// Skipped series returned per interval by the Admin surface.
pub const ADMIN_SKIPPED_SERIES_LIMIT: i64 = 20;
/// Intervals returned by the Admin surface.
pub const ADMIN_RECENT_INTERVAL_LIMIT: i64 = 10;

/// Operator-declared capacity policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityConfig {
    enabled: bool,
    pause_below_bytes: Option<u64>,
    resume_above_bytes: Option<u64>,
    sample_interval_seconds: u64,
    origin: Option<PathBuf>,
}

impl CapacityConfig {
    /// A policy that only reports capacity and never pauses history.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            pause_below_bytes: None,
            resume_above_bytes: None,
            sample_interval_seconds: DEFAULT_SAMPLE_INTERVAL_SECONDS,
            origin: None,
        }
    }

    /// The largest threshold the Server can persist: SQLite integers are
    /// signed 64-bit, so a bigger byte count would not survive a round trip.
    pub const MAX_PERSISTED_BYTES: u64 = i64::MAX as u64;

    /// Build a policy from declared values.
    ///
    /// Validation is deliberately strict: enabling protection without
    /// thresholds is refused rather than defaulted, because the Server has no
    /// measured basis for a number on someone else's disk.
    pub fn from_declared(
        enabled: bool,
        pause_below_bytes: Option<u64>,
        resume_above_bytes: Option<u64>,
        sample_interval_seconds: Option<u64>,
        origin: Option<PathBuf>,
    ) -> Result<Self, String> {
        let sample_interval_seconds =
            sample_interval_seconds.unwrap_or(DEFAULT_SAMPLE_INTERVAL_SECONDS);
        if !(MIN_SAMPLE_INTERVAL_SECONDS..=MAX_SAMPLE_INTERVAL_SECONDS)
            .contains(&sample_interval_seconds)
        {
            return Err(format!(
                "sample_interval_seconds must be between {MIN_SAMPLE_INTERVAL_SECONDS} and {MAX_SAMPLE_INTERVAL_SECONDS}"
            ));
        }
        if !enabled {
            return Ok(Self {
                enabled,
                pause_below_bytes,
                resume_above_bytes,
                sample_interval_seconds,
                origin,
            });
        }
        let pause_below_bytes = pause_below_bytes.ok_or_else(|| {
            "enabled capacity protection requires pause_below_bytes; no default is invented because the threshold must be measured for this deployment"
                .to_owned()
        })?;
        let resume_above_bytes = resume_above_bytes.ok_or_else(|| {
            "enabled capacity protection requires resume_above_bytes; it is the free-space level at which optional history may resume"
                .to_owned()
        })?;
        if pause_below_bytes == 0 {
            return Err("pause_below_bytes must be greater than zero".to_owned());
        }
        if resume_above_bytes < pause_below_bytes {
            return Err(
                "resume_above_bytes must be greater than or equal to pause_below_bytes".to_owned(),
            );
        }
        // SQLite records a byte threshold as a signed 64-bit integer, so a
        // larger value could not be stored and read back unchanged. Refusing
        // it here keeps the persisted evidence identical to what was declared.
        if pause_below_bytes > Self::MAX_PERSISTED_BYTES
            || resume_above_bytes > Self::MAX_PERSISTED_BYTES
        {
            return Err(format!(
                "capacity thresholds must not exceed {} bytes, the largest value the Server can record",
                Self::MAX_PERSISTED_BYTES
            ));
        }
        Ok(Self {
            enabled,
            pause_below_bytes: Some(pause_below_bytes),
            resume_above_bytes: Some(resume_above_bytes),
            sample_interval_seconds,
            origin,
        })
    }

    /// Whether low-space protection may pause optional history.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Free-space floor (bytes) below which optional history pauses.
    pub fn pause_below_bytes(&self) -> Option<u64> {
        self.pause_below_bytes
    }

    /// Free-space level (bytes) at or above which optional history resumes.
    pub fn resume_above_bytes(&self) -> Option<u64> {
        self.resume_above_bytes
    }

    /// Sampling cadence in seconds.
    pub fn sample_interval_seconds(&self) -> u64 {
        self.sample_interval_seconds
    }

    /// Config file that declared this policy, when one did.
    pub fn origin(&self) -> Option<&Path> {
        self.origin.as_deref()
    }
}

/// One measurement of the filesystem that holds the Server database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemSample {
    /// Directory that was measured (the Server database's parent).
    pub mount_path: PathBuf,
    /// Filesystem size in bytes.
    pub total_bytes: u64,
    /// Bytes available to the Server user.
    pub available_bytes: u64,
}

/// Measure the filesystem containing "path".
///
/// nix::sys::statvfs reports f_bavail, which is the space available to an
/// unprivileged process, not f_bfree (which includes root-reserved blocks the
/// Server cannot actually spend).
#[cfg(unix)]
pub fn sample_filesystem(path: &Path) -> Result<FilesystemSample, String> {
    let stats = nix::sys::statvfs::statvfs(path)
        .map_err(|error| format!("statvfs({}) failed: {error}", path.display()))?;
    let fragment = stats.fragment_size();
    let unit = if fragment > 0 {
        fragment as u64
    } else {
        stats.block_size() as u64
    };
    Ok(FilesystemSample {
        mount_path: path.to_path_buf(),
        total_bytes: unit.saturating_mul(stats.blocks() as u64),
        available_bytes: unit.saturating_mul(stats.blocks_available() as u64),
    })
}

/// Capacity sampling is not implemented outside Unix; the Server keeps
/// reporting sampling_error and never pauses history there.
#[cfg(not(unix))]
pub fn sample_filesystem(path: &Path) -> Result<FilesystemSample, String> {
    Err(format!(
        "filesystem capacity sampling is unsupported on this platform ({})",
        path.display()
    ))
}

/// Pure hysteresis decision: should optional history be paused?
///
/// Entering protection needs free space strictly below the floor; leaving it
/// needs free space at or above the release level. A single threshold would
/// flap between the two states on every sample, which is exactly the pattern
/// that turns one crowded disk into an unreadable history.
pub fn protection_required(
    currently_protected: bool,
    available_bytes: u64,
    pause_below_bytes: u64,
    resume_above_bytes: u64,
) -> bool {
    if currently_protected {
        available_bytes < resume_above_bytes
    } else {
        available_bytes < pause_below_bytes
    }
}

/// What an ingestion should do with optional metric history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryGate {
    /// Optional history is recorded normally.
    Record,
    /// Protection is active: optional samples are not written, and the gap is
    /// recorded against this interval instead.
    Paused { interval_id: String },
}

/// Kind of optional series a skipped write belonged to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkippedScope {
    /// A per-node series (node_metric_samples).
    Node,
    /// A per-Agent host series (host_metric_samples).
    Host,
}

impl SkippedScope {
    /// Stable database/API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SkippedScope::Node => "node",
            SkippedScope::Host => "host",
        }
    }
}

/// Current capacity picture, for the Admin surface, Doctor and metrics.
#[derive(Debug, Clone)]
pub struct CapacityStatus {
    /// Whether an enabled policy exists.
    pub enabled: bool,
    /// Whether optional history is currently paused.
    pub protected: bool,
    /// Interval that optional history is paused against.
    pub active_interval_id: Option<String>,
    /// Latest successful measurement.
    pub sample: Option<FilesystemSample>,
    /// When the latest successful measurement was taken.
    pub sampled_at: Option<String>,
    /// Why the latest measurement failed, if it did.
    pub sampling_error: Option<String>,
    /// Why the last protection transition could not be recorded, if it could
    /// not: the in-memory state stays on the previous value and is retried.
    pub transition_error: Option<String>,
}

/// A protection interval as stored, with its skipped-series evidence.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityIntervalRecord {
    pub interval_id: String,
    pub source_mount: String,
    pub started_at: String,
    pub started_reason: String,
    pub opened_total_bytes: u64,
    pub opened_available_bytes: u64,
    pub pause_below_bytes: u64,
    pub resume_above_bytes: u64,
    pub ended_at: Option<String>,
    pub ended_reason: Option<String>,
    pub resumed_total_bytes: Option<u64>,
    pub resumed_available_bytes: Option<u64>,
    pub updated_at: String,
    /// Total optional samples skipped during this interval.
    pub skipped_sample_count: i64,
    /// Number of distinct series with skipped samples.
    pub skipped_series_total: i64,
    /// Bounded per-series detail, ordered by skipped samples.
    pub skipped_series: Vec<CapacitySkippedSeries>,
}

/// One series that lost optional samples while protection was active.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacitySkippedSeries {
    pub scope_kind: String,
    pub scope_key: String,
    pub metric: String,
    pub skipped_count: i64,
    pub first_skipped_at: String,
    pub last_skipped_at: String,
}

/// Failure to read or write protection state.
#[derive(Debug, Error)]
pub enum CapacityError {
    /// The Server database rejected the protection bookkeeping.
    #[error("capacity protection storage failed: {0}")]
    Storage(#[from] sqlx::Error),
}

struct ProtectionState {
    protected: bool,
    interval_id: Option<String>,
    sample: Option<FilesystemSample>,
    sampled_at: Option<String>,
    sampling_error: Option<String>,
    transition_error: Option<String>,
    /// True until this process has successfully read the durable open interval.
    ///
    /// Startup reconciliation adopts an interval a previous process left open,
    /// but the read can fail on its own (a locked or briefly unavailable
    /// database). While it has never succeeded, a tick retries it before
    /// deciding anything: treating "I could not read the record" as "no
    /// interval is open" would let optional history resume under pressure and
    /// leave the recorded interval open forever.
    adoption_pending: bool,
}

/// Whether a sampling tick changed the protection state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectionTransition {
    /// Nothing changed.
    Unchanged,
    /// Protection started; optional history is now paused.
    Opened { interval_id: String },
    /// Protection ended; optional history resumes.
    Closed { interval_id: String, reason: String },
}

/// Capacity visibility and low-space protection for one Server process.
pub struct CapacityProtection {
    config: CapacityConfig,
    mount_path: Option<PathBuf>,
    state: RwLock<ProtectionState>,
    tick: tokio::sync::Mutex<()>,
}

impl CapacityProtection {
    /// Protection for a Server database at "database_path".
    pub fn new(config: CapacityConfig, database_path: Option<&Path>) -> Self {
        let mount_path = database_path
            .and_then(Path::parent)
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
        Self {
            config,
            mount_path,
            state: RwLock::new(ProtectionState {
                protected: false,
                interval_id: None,
                sample: None,
                sampled_at: None,
                sampling_error: None,
                transition_error: None,
                adoption_pending: true,
            }),
            tick: tokio::sync::Mutex::new(()),
        }
    }

    /// A policy that only reports capacity, never pauses history.
    pub fn disabled(database_path: Option<&Path>) -> Self {
        Self::new(CapacityConfig::disabled(), database_path)
    }

    /// This process's declared policy.
    pub fn config(&self) -> &CapacityConfig {
        &self.config
    }

    /// Directory whose filesystem is measured.
    pub fn mount_path(&self) -> Option<&Path> {
        self.mount_path.as_deref()
    }

    /// Current capacity picture.
    pub fn status(&self) -> CapacityStatus {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        CapacityStatus {
            enabled: self.config.enabled,
            protected: state.protected,
            active_interval_id: state.interval_id.clone(),
            sample: state.sample.clone(),
            sampled_at: state.sampled_at.clone(),
            sampling_error: state.sampling_error.clone(),
            transition_error: state.transition_error.clone(),
        }
    }

    /// What an ingestion should do with optional history right now.
    ///
    /// This reads process memory only: ingestion already holds the single
    /// SQLite write connection inside its transaction, so asking the database
    /// here would be a self-deadlock, not a consistency win.
    pub fn history_gate(&self) -> HistoryGate {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        match (&state.protected, &state.interval_id) {
            (true, Some(interval_id)) => HistoryGate::Paused {
                interval_id: interval_id.clone(),
            },
            _ => HistoryGate::Record,
        }
    }

    /// Adopt protection left open by a previous process, then sample once.
    ///
    /// A Server that restarts during low space must not silently resume
    /// optional history, so an open interval is adopted before the first
    /// measurement. The interval keeps the thresholds that were in force when
    /// it opened; today's policy decides whether it may close.
    pub async fn reconcile(
        &self,
        pool: &SqlitePool,
    ) -> Result<ProtectionTransition, CapacityError> {
        self.adopt_open_interval(pool)
            .await
            .map_err(CapacityError::Storage)?;
        self.check_now(pool).await
    }

    /// Adopt the interval a previous process left open, if any.
    ///
    /// The pending flag clears only once the read succeeds, so a startup
    /// reconciliation that failed on its own is retried by the next tick. A
    /// process that cannot read the record must not conclude that no interval
    /// is open: it would resume optional history under pressure and leave the
    /// recorded interval open forever.
    async fn adopt_open_interval(&self, pool: &SqlitePool) -> Result<(), sqlx::Error> {
        let open = load_open_interval(pool).await?;
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(open) = open {
            state.protected = true;
            state.interval_id = Some(open.interval_id);
        }
        state.adoption_pending = false;
        Ok(())
    }

    /// Whether this process still has to read the durable open interval.
    fn adoption_pending(&self) -> bool {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        state.adoption_pending
    }

    /// Sample the filesystem once and record any protection transition.
    ///
    /// The durable write happens before the in-memory flag, so a crash between
    /// them leaves an open interval (history stays paused on restart) rather
    /// than a skipped stretch of history with no record of it. If the write
    /// fails the in-memory state is left alone and retried on the next tick.
    pub async fn check_now(
        &self,
        pool: &SqlitePool,
    ) -> Result<ProtectionTransition, CapacityError> {
        let _tick = self.tick.lock().await;
        if self.adoption_pending() {
            if let Err(error) = self.adopt_open_interval(pool).await {
                // Keep sampling: the ticket's rule is that pressure never
                // stops the clock. The gate stays as it is for this tick, the
                // failed adoption stays visible, and the next tick retries it.
                self.record_transition_error(&error);
            }
        }
        let Some(mount_path) = self.mount_path.clone() else {
            self.record_measurement(
                None,
                None,
                Some("the Server database has no parent directory to measure".to_owned()),
            );
            return Ok(ProtectionTransition::Unchanged);
        };
        let sample = match sample_filesystem(&mount_path) {
            Ok(sample) => sample,
            Err(error) => {
                self.record_measurement(None, None, Some(error));
                return Ok(ProtectionTransition::Unchanged);
            }
        };
        let sampled_at = format_rfc3339(now_utc());
        let (protected, interval_id) = {
            let state = self.state.read().unwrap_or_else(|error| error.into_inner());
            (state.protected, state.interval_id.clone())
        };
        let required = match (
            self.config.enabled,
            self.config.pause_below_bytes,
            self.config.resume_above_bytes,
        ) {
            (true, Some(pause_below_bytes), Some(resume_above_bytes)) => protection_required(
                protected,
                sample.available_bytes,
                pause_below_bytes,
                resume_above_bytes,
            ),
            _ => false,
        };

        if required == protected {
            self.record_measurement(Some(sample), Some(sampled_at), None);
            return Ok(ProtectionTransition::Unchanged);
        }

        if required {
            let pause_below_bytes = self.config.pause_below_bytes.unwrap_or_default();
            let resume_above_bytes = self.config.resume_above_bytes.unwrap_or_default();
            let interval_id = uuid::Uuid::new_v4().to_string();
            let open = OpenInterval {
                interval_id: &interval_id,
                source_mount: &mount_path.to_string_lossy(),
                started_at: &sampled_at,
                opened: &sample,
                pause_below_bytes,
                resume_above_bytes,
            };
            match open_interval(pool, &open).await {
                Ok(()) => {
                    let mut state = self
                        .state
                        .write()
                        .unwrap_or_else(|error| error.into_inner());
                    state.protected = true;
                    state.interval_id = Some(interval_id.clone());
                    state.sample = Some(sample);
                    state.sampled_at = Some(sampled_at);
                    state.sampling_error = None;
                    state.transition_error = None;
                    Ok(ProtectionTransition::Opened { interval_id })
                }
                Err(error) => {
                    self.record_measurement(Some(sample), Some(sampled_at), None);
                    self.record_transition_error(&error);
                    Err(CapacityError::Storage(error))
                }
            }
        } else {
            let reason = if self.config.enabled {
                "resumed"
            } else {
                "protection_disabled"
            };
            let Some(interval_id) = interval_id else {
                // No recorded interval to close: clear memory and let the next
                // sample decide again.
                let mut state = self
                    .state
                    .write()
                    .unwrap_or_else(|error| error.into_inner());
                state.protected = false;
                state.sample = Some(sample);
                state.sampled_at = Some(sampled_at);
                state.sampling_error = None;
                return Ok(ProtectionTransition::Unchanged);
            };
            match close_interval(pool, &interval_id, reason, &sampled_at, &sample).await {
                Ok(()) => {
                    let mut state = self
                        .state
                        .write()
                        .unwrap_or_else(|error| error.into_inner());
                    state.protected = false;
                    state.interval_id = None;
                    state.sample = Some(sample);
                    state.sampled_at = Some(sampled_at);
                    state.sampling_error = None;
                    state.transition_error = None;
                    Ok(ProtectionTransition::Closed {
                        interval_id,
                        reason: reason.to_owned(),
                    })
                }
                Err(error) => {
                    self.record_measurement(Some(sample), Some(sampled_at), None);
                    self.record_transition_error(&error);
                    Err(CapacityError::Storage(error))
                }
            }
        }
    }

    /// Sample on the configured cadence until shutdown fires.
    /// Sample on the declared cadence until the shutdown future resolves.
    ///
    /// The caller supplies the shutdown signal as a future, so this module
    /// never needs to know about Server state; the Server passes its own
    /// shutdown signal.
    pub async fn run_worker<F>(self: std::sync::Arc<Self>, pool: SqlitePool, shutdown: F)
    where
        F: std::future::Future<Output = ()>,
    {
        let every = Duration::from_secs(self.config.sample_interval_seconds);
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = ticker.tick() => {}
            }
            if let Err(error) = self.check_now(&pool).await {
                eprintln!(
                    "capacity protection transition could not be recorded: {}",
                    crate::redaction::redact_sensitive(&error.to_string())
                );
            }
        }
    }

    fn record_measurement(
        &self,
        sample: Option<FilesystemSample>,
        sampled_at: Option<String>,
        sampling_error: Option<String>,
    ) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(sample) = sample {
            state.sample = Some(sample);
        }
        if let Some(sampled_at) = sampled_at {
            state.sampled_at = Some(sampled_at);
        }
        state.sampling_error = sampling_error;
    }

    fn record_transition_error(&self, error: &sqlx::Error) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        state.transition_error = Some(error.to_string());
    }
}

/// Details needed to record a new protection interval.
struct OpenInterval<'a> {
    interval_id: &'a str,
    source_mount: &'a str,
    started_at: &'a str,
    opened: &'a FilesystemSample,
    pause_below_bytes: u64,
    resume_above_bytes: u64,
}

/// An interval left open by a previous Server process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenIntervalRecord {
    /// Interval identifier that is still open.
    pub interval_id: String,
    /// When protection started.
    pub started_at: String,
}

/// Read the interval that is still open, if any.
pub async fn load_open_interval(
    pool: &SqlitePool,
) -> Result<Option<OpenIntervalRecord>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT interval_id, started_at FROM capacity_protection_intervals WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(interval_id, started_at)| OpenIntervalRecord {
        interval_id,
        started_at,
    }))
}

/// Record the start of protection in one statement.
///
/// The thresholds in force are copied into the row: an interval must be able
/// to explain itself even after the config file changes. Nothing here (or
/// anywhere else in this module) prunes evidence: an interval and the gap it
/// explains are history the Server promised to keep, so protection must never
/// quietly delete rows to stay small (design §11.5, issue #212). A single
/// statement also means the transition cannot half-commit — the interval row
/// and the in-memory gate either both move or neither does.
async fn open_interval(pool: &SqlitePool, open: &OpenInterval<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO capacity_protection_intervals (interval_id, source_mount, started_at, started_reason, opened_total_bytes, opened_available_bytes, pause_below_bytes, resume_above_bytes, ended_at, ended_reason, resumed_total_bytes, resumed_available_bytes, created_at, updated_at) VALUES (?, ?, ?, 'low_space', ?, ?, ?, ?, NULL, NULL, NULL, NULL, ?, ?)",
    )
    .bind(open.interval_id)
    .bind(open.source_mount)
    .bind(open.started_at)
    .bind(saturating_i64(open.opened.total_bytes))
    .bind(saturating_i64(open.opened.available_bytes))
    .bind(saturating_i64(open.pause_below_bytes))
    .bind(saturating_i64(open.resume_above_bytes))
    .bind(open.started_at)
    .bind(open.started_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that protection ended, with the measurement that justified it.
async fn close_interval(
    pool: &SqlitePool,
    interval_id: &str,
    reason: &str,
    ended_at: &str,
    resumed: &FilesystemSample,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE capacity_protection_intervals SET ended_at = ?, ended_reason = ?, resumed_total_bytes = ?, resumed_available_bytes = ?, updated_at = ? WHERE interval_id = ? AND ended_at IS NULL",
    )
    .bind(ended_at)
    .bind(reason)
    .bind(saturating_i64(resumed.total_bytes))
    .bind(saturating_i64(resumed.available_bytes))
    .bind(ended_at)
    .bind(interval_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record one optional sample that protection refused to write.
///
/// This runs inside the caller's ingestion transaction. That is the point: the
/// decision "this sample was not stored because of protection" commits with the
/// Report receipt that carried it. If the entry cannot be written the whole
/// Report rolls back and the Agent retries, so a skipped sample is never both
/// unrecorded and unrecoverable.
///
/// The count follows the same identity rule as the history writer, which keys
/// its rows on (scope, metric, observed_at): re-sending a reading that this
/// series already recorded as skipped is not a second lost sample. It advances
/// only when the refused reading is newer than every reading already counted
/// for the series, which is exactly the condition under which the recorded
/// "last skipped" mark moves. So a replay never counts twice, and the number
/// never claims more lost readings than the series accounted for. A
/// late-arriving *older* reading does not advance it, because telling one apart
/// from a replay would need one ledger row per skipped reading, costing at
/// least as much disk as the history the pause is protecting.
pub async fn record_skipped_series(
    tx: &mut Transaction<'_, Sqlite>,
    interval_id: &str,
    scope: SkippedScope,
    scope_key: &str,
    metric: &str,
    observed_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO capacity_skipped_series (interval_id, scope_kind, scope_key, metric, skipped_count, first_skipped_at, last_skipped_at) VALUES (?, ?, ?, ?, 1, ?, ?) ON CONFLICT(interval_id, scope_kind, scope_key, metric) DO UPDATE SET skipped_count = capacity_skipped_series.skipped_count + 1, first_skipped_at = MIN(capacity_skipped_series.first_skipped_at, excluded.first_skipped_at), last_skipped_at = MAX(capacity_skipped_series.last_skipped_at, excluded.last_skipped_at) WHERE excluded.last_skipped_at > capacity_skipped_series.last_skipped_at",
    )
    .bind(interval_id)
    .bind(scope.as_str())
    .bind(scope_key)
    .bind(metric)
    .bind(observed_at)
    .bind(observed_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Protection intervals for the Admin surface, newest first.
pub async fn recent_intervals(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<CapacityIntervalRecord>, sqlx::Error> {
    type IntervalRow = (
        String,
        String,
        String,
        String,
        i64,
        i64,
        i64,
        i64,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        String,
    );
    let rows = sqlx::query_as::<_, IntervalRow>(
        "SELECT interval_id, source_mount, started_at, started_reason, opened_total_bytes, opened_available_bytes, pause_below_bytes, resume_above_bytes, ended_at, ended_reason, resumed_total_bytes, resumed_available_bytes, updated_at FROM capacity_protection_intervals ORDER BY started_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        let (
            interval_id,
            source_mount,
            started_at,
            started_reason,
            opened_total_bytes,
            opened_available_bytes,
            pause_below_bytes,
            resume_above_bytes,
            ended_at,
            ended_reason,
            resumed_total_bytes,
            resumed_available_bytes,
            updated_at,
        ) = row;
        let (skipped_series_total, skipped_sample_count): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(skipped_count), 0) FROM capacity_skipped_series WHERE interval_id = ?",
        )
        .bind(&interval_id)
        .fetch_one(pool)
        .await?;
        let series = sqlx::query_as::<_, (String, String, String, i64, String, String)>(
            "SELECT scope_kind, scope_key, metric, skipped_count, first_skipped_at, last_skipped_at FROM capacity_skipped_series WHERE interval_id = ? ORDER BY skipped_count DESC, last_skipped_at DESC, scope_key ASC, metric ASC LIMIT ?",
        )
        .bind(&interval_id)
        .bind(ADMIN_SKIPPED_SERIES_LIMIT)
        .fetch_all(pool)
        .await?;
        records.push(CapacityIntervalRecord {
            interval_id,
            source_mount,
            started_at,
            started_reason,
            opened_total_bytes: unsigned(opened_total_bytes),
            opened_available_bytes: unsigned(opened_available_bytes),
            pause_below_bytes: unsigned(pause_below_bytes),
            resume_above_bytes: unsigned(resume_above_bytes),
            ended_at,
            ended_reason,
            resumed_total_bytes: resumed_total_bytes.map(unsigned),
            resumed_available_bytes: resumed_available_bytes.map(unsigned),
            updated_at,
            skipped_sample_count,
            skipped_series_total,
            skipped_series: series
                .into_iter()
                .map(
                    |(
                        scope_kind,
                        scope_key,
                        metric,
                        skipped_count,
                        first_skipped_at,
                        last_skipped_at,
                    )| {
                        CapacitySkippedSeries {
                            scope_kind,
                            scope_key,
                            metric,
                            skipped_count,
                            first_skipped_at,
                            last_skipped_at,
                        }
                    },
                )
                .collect(),
        });
    }
    Ok(records)
}

/// Byte counts are stored as SQLite INTEGER; saturation keeps an operator's
/// larger-than-i64 threshold readable instead of negative.
fn saturating_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn unsigned(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(enabled: bool, pause: Option<u64>, resume: Option<u64>) -> CapacityConfig {
        CapacityConfig::from_declared(enabled, pause, resume, None, None).unwrap()
    }

    /// A policy whose floor no filesystem can reach, so a transition happens
    /// deterministically without waiting for a real disk to fill up.
    fn forced_policy() -> CapacityConfig {
        let floor = CapacityConfig::MAX_PERSISTED_BYTES;
        policy(true, Some(floor), Some(floor))
    }

    async fn database(dir: &Path) -> crate::database::ServerDatabase {
        crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.join("server.db"),
        ))
        .await
        .unwrap()
    }

    #[test]
    fn enabled_policy_refuses_invented_thresholds() {
        let error = CapacityConfig::from_declared(true, None, Some(1), None, None).unwrap_err();
        assert!(error.contains("pause_below_bytes"), "{error}");
        let error = CapacityConfig::from_declared(true, Some(1), None, None, None).unwrap_err();
        assert!(error.contains("resume_above_bytes"), "{error}");
        let error = CapacityConfig::from_declared(true, Some(0), Some(1), None, None).unwrap_err();
        assert!(error.contains("greater than zero"), "{error}");
    }

    #[test]
    fn release_level_must_not_be_below_the_pause_floor() {
        let error =
            CapacityConfig::from_declared(true, Some(2_000), Some(1_000), None, None).unwrap_err();
        assert!(error.contains("resume_above_bytes"), "{error}");
        let equal = CapacityConfig::from_declared(true, Some(1_000), Some(1_000), None, None);
        assert!(equal.is_ok());
    }

    #[test]
    fn sampling_cadence_is_bounded() {
        let slow =
            CapacityConfig::from_declared(false, None, None, Some(86_401), None).unwrap_err();
        assert!(slow.contains("sample_interval_seconds"), "{slow}");
        let fast = CapacityConfig::from_declared(false, None, None, Some(4), None).unwrap_err();
        assert!(fast.contains("sample_interval_seconds"), "{fast}");
        assert_eq!(
            policy(false, None, None).sample_interval_seconds(),
            DEFAULT_SAMPLE_INTERVAL_SECONDS
        );
    }

    #[test]
    fn hysteresis_holds_protection_between_the_two_levels() {
        assert!(protection_required(false, 99, 100, 200));
        assert!(!protection_required(false, 100, 100, 200));
        assert!(protection_required(true, 199, 100, 200));
        assert!(!protection_required(true, 200, 100, 200));
        // Equal thresholds still have one defined answer in each state, so a
        // zero-hysteresis policy is well defined rather than oscillating.
        assert!(protection_required(false, 99, 100, 100));
        assert!(!protection_required(true, 100, 100, 100));
    }

    #[test]
    fn mount_path_is_the_database_parent() {
        let protection =
            CapacityProtection::disabled(Some(Path::new("/var/lib/platpulse/server.db")));
        assert_eq!(
            protection.mount_path(),
            Some(Path::new("/var/lib/platpulse"))
        );
        assert_eq!(protection.history_gate(), HistoryGate::Record);
        assert!(!protection.status().enabled);
        assert_eq!(CapacityProtection::disabled(None).mount_path(), None);
    }

    #[test]
    fn filesystem_sample_reports_real_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let sample = sample_filesystem(dir.path()).unwrap();
        assert_eq!(sample.mount_path, dir.path());
        assert!(sample.total_bytes > 0, "{sample:?}");
        assert!(sample.available_bytes <= sample.total_bytes, "{sample:?}");
    }

    #[tokio::test]
    async fn low_space_opens_one_interval_and_keeps_it_while_space_is_short() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("server.db");
        let database = database(dir.path()).await;
        let pool = database.pool();
        // A floor no filesystem can reach forces the transition
        // deterministically. It is the largest threshold the Server records, so
        // the round trip back out of SQLite is exact.
        let floor = CapacityConfig::MAX_PERSISTED_BYTES;
        let protection = CapacityProtection::new(forced_policy(), Some(&db_path));

        let opened = protection.check_now(pool).await.unwrap();
        let ProtectionTransition::Opened { interval_id } = opened else {
            panic!("expected protection to open: {opened:?}");
        };
        assert_eq!(
            protection.history_gate(),
            HistoryGate::Paused {
                interval_id: interval_id.clone()
            }
        );
        assert_eq!(
            protection.status().active_interval_id.as_deref(),
            Some(interval_id.as_str())
        );
        assert!(protection.status().sample.is_some());
        // The interval records the measurement that opened it; a later tick
        // samples again, so capture the opening sample now.
        let opened_available = protection.status().sample.as_ref().unwrap().available_bytes;

        // A second tick inside the hysteresis band changes nothing and does not
        // open a second interval.
        assert_eq!(
            protection.check_now(pool).await.unwrap(),
            ProtectionTransition::Unchanged
        );
        let records = recent_intervals(pool, ADMIN_RECENT_INTERVAL_LIMIT)
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].interval_id, interval_id);
        assert_eq!(records[0].started_reason, "low_space");
        assert_eq!(records[0].ended_at, None);
        assert_eq!(records[0].pause_below_bytes, floor);
        assert_eq!(records[0].opened_available_bytes, opened_available);
        assert!(records[0].opened_available_bytes < floor);

        // A restart adopts the open interval before sampling, so history stays
        // paused until the filesystem actually recovers.
        let restarted = CapacityProtection::new(forced_policy(), Some(&db_path));
        assert_eq!(restarted.history_gate(), HistoryGate::Record);
        assert_eq!(
            restarted.reconcile(pool).await.unwrap(),
            ProtectionTransition::Unchanged
        );
        assert_eq!(
            restarted.history_gate(),
            HistoryGate::Paused { interval_id }
        );
        database.close().await;
    }

    #[tokio::test]
    async fn recovery_closes_the_interval_with_the_measurement_that_released_it() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("server.db");
        let database = database(dir.path()).await;
        let pool = database.pool();
        let paused = CapacityProtection::new(forced_policy(), Some(&db_path));
        let ProtectionTransition::Opened { interval_id } = paused.check_now(pool).await.unwrap()
        else {
            panic!("expected protection to open");
        };

        // Any real disk has more than one byte free, so this policy resumes.
        let recovered = CapacityProtection::new(policy(true, Some(1), Some(1)), Some(&db_path));
        assert_eq!(
            recovered.reconcile(pool).await.unwrap(),
            ProtectionTransition::Closed {
                interval_id: interval_id.clone(),
                reason: "resumed".to_owned()
            }
        );
        assert_eq!(recovered.history_gate(), HistoryGate::Record);
        let records = recent_intervals(pool, ADMIN_RECENT_INTERVAL_LIMIT)
            .await
            .unwrap();
        assert_eq!(records[0].ended_reason.as_deref(), Some("resumed"));
        assert!(records[0].ended_at.is_some());
        assert!(records[0].resumed_available_bytes.unwrap() > 1);

        // Turning the policy off closes an interval too, and says why.
        let paused_again = CapacityProtection::new(forced_policy(), Some(&db_path));
        let ProtectionTransition::Opened {
            interval_id: second,
        } = paused_again.check_now(pool).await.unwrap()
        else {
            panic!("expected protection to open again");
        };
        let disabled = CapacityProtection::disabled(Some(&db_path));
        assert_eq!(
            disabled.reconcile(pool).await.unwrap(),
            ProtectionTransition::Closed {
                interval_id: second,
                reason: "protection_disabled".to_owned()
            }
        );
        assert!(!disabled.status().protected);
        database.close().await;
    }

    #[tokio::test]
    async fn skipped_series_aggregate_the_gap_inside_the_ingestion_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("server.db");
        let database = database(dir.path()).await;
        let pool = database.pool();
        let protection = CapacityProtection::new(forced_policy(), Some(&db_path));
        let ProtectionTransition::Opened { interval_id } =
            protection.check_now(pool).await.unwrap()
        else {
            panic!("expected protection to open");
        };

        let mut tx = pool.begin().await.unwrap();
        record_skipped_series(
            &mut tx,
            &interval_id,
            SkippedScope::Node,
            "0195f2a1-0014-4014-8014-000000000014",
            "process_cpu_percent",
            "2026-08-12T09:59:55Z",
        )
        .await
        .unwrap();
        record_skipped_series(
            &mut tx,
            &interval_id,
            SkippedScope::Node,
            "0195f2a1-0014-4014-8014-000000000014",
            "process_cpu_percent",
            "2026-08-12T10:00:05Z",
        )
        .await
        .unwrap();
        record_skipped_series(
            &mut tx,
            &interval_id,
            SkippedScope::Host,
            "0195f2a1-0011-4011-8011-000000000011",
            "network_rx_bytes_per_sec",
            "2026-08-12T10:00:05Z",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let records = recent_intervals(pool, ADMIN_RECENT_INTERVAL_LIMIT)
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].skipped_sample_count, 3);
        assert_eq!(records[0].skipped_series_total, 2);
        assert_eq!(records[0].skipped_series.len(), 2);
        assert_eq!(records[0].skipped_series[0].scope_kind, "node");
        assert_eq!(records[0].skipped_series[0].skipped_count, 2);
        assert_eq!(
            records[0].skipped_series[0].first_skipped_at,
            "2026-08-12T09:59:55Z"
        );
        assert_eq!(
            records[0].skipped_series[0].last_skipped_at,
            "2026-08-12T10:00:05Z"
        );
        assert_eq!(records[0].skipped_series[1].scope_kind, "host");

        // Rolling back the ingestion transaction removes the gap record with
        // the rest of that Report's writes.
        let mut tx = pool.begin().await.unwrap();
        record_skipped_series(
            &mut tx,
            &interval_id,
            SkippedScope::Node,
            "0195f2a1-0014-4014-8014-000000000014",
            "peer_inbound_count",
            "2026-08-12T10:00:15Z",
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        let records = recent_intervals(pool, ADMIN_RECENT_INTERVAL_LIMIT)
            .await
            .unwrap();
        assert_eq!(records[0].skipped_sample_count, 3);
        assert_eq!(records[0].skipped_series_total, 2);
        database.close().await;
    }
}
