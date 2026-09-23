//! The frozen-v1 upgrade-preparation bridge (issue #189).
//!
//! [ADR 0007](../../docs/adr/0007-server-managed-inventory-revision.md) requires
//! a coordinated v1 -> v2 cutover: under v1 an operator stops ordinary Report
//! generation, drains the immutable backlog, completes the final Closing, and
//! proves that the declaration used by that Closing is exactly what the Server
//! last accepted. Only then may a later checkpoint convert the Server baseline
//! and the Agent confirmation record. The conversion itself is future work; this
//! module delivers the preparation half as one operator-facing entry point.
//!
//! The bridge deliberately reuses the existing immutable-delivery machinery:
//! Reports are the same persisted bytes, sent with the same transport, and
//! acknowledged by the same receipt-application transaction. It never rewrites a
//! Report, never discards unacknowledged backlog, and never uses `recover`'s
//! stale-Closing quarantine to skip the gate. On success it writes one bounded
//! evidence row and leaves the Store in the post-Closing `drained_pending`
//! state, without starting normal collection.
//!
//! Failure is actionable and safe to retry: a Closing still in the spool is
//! resumed with its original bytes rather than replaced, so a transport outage
//! does not accumulate duplicate Closing reports.

use std::str::FromStr;
use std::time::Duration;

use platpulse_core::identity::{AgentId, BootId};
use platpulse_core::inventory::NodeInventory;
use platpulse_core::{AgentReport, BootTransition, InventoryDisposition, ReceiptDisposition};
use serde::Deserialize;
use sqlx::Connection;
use thiserror::Error;

use crate::config::AgentConfig;
use crate::credential::load_credential_file;
use crate::database::{
    AgentDatabaseConfig, AgentRuntimeLock, AgentStore, AgentStoreWritePermit, now_rfc3339,
};

/// Agent API path of the Server-owned preparation baseline.
pub const AGENT_PREPARATION_PATH: &str = "/api/agent/v1/preparation";

/// Bound the read of the Server baseline so a hung or unreachable Server fails
/// with an actionable retry instead of blocking the operator indefinitely. The
/// baseline is a read of already-committed state, so a short bound is safe; a
/// timeout never invalidates the completed Closing or the pending Reports.
const PREPARATION_BASELINE_TIMEOUT: Duration = Duration::from_secs(30);

/// The verified result of one successful v1 preparation, for the later
/// coordinated checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparationOutcome {
    pub closing_report_id: String,
    pub inventory_revision: u64,
    pub inventory_sha256: String,
    pub closing_receipt_disposition: &'static str,
    pub closed_boot_id: String,
    pub next_boot_id: String,
    pub pending_transition: &'static str,
    pub verified_at: String,
}

#[derive(Debug, Error)]
pub enum PreparationError {
    #[error(transparent)]
    Config(#[from] crate::config::AgentConfigError),
    #[error(
        "the Agent runtime owns this Store; stop `platpulse-agent run` before preparing the upgrade ({0})"
    )]
    RuntimeOwned(String),
    #[error(
        "this configuration is already Server-managed (v2); there is no frozen v1 revision to prepare"
    )]
    AlreadyServerManaged,
    #[error("Agent is not enrolled")]
    NotEnrolled,
    #[error("Agent Store initialization failed: {0}")]
    Store(#[from] crate::database::AgentDatabaseError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(
        "a previous shutdown or preparation is incomplete (boot_state={0}); finish it with `platpulse-agent recover` before retrying"
    )]
    RecoveryRequired(String),
    #[error("report delivery failed: {0}")]
    Delivery(#[from] crate::reporting::ReportStoreError),
    #[error(
        "the final Closing was not delivered before the sender deadline; re-run `prepare-upgrade` to resume it"
    )]
    ClosingDeadline,
    #[error(
        "the final Closing report {0} was refused by the Server; fix the reported reason in v1 and re-run"
    )]
    ClosingRejected(String),
    #[error(
        "the Server accepted the Closing report {0} but refused its whole Inventory; fix the Inventory declaration and re-run in v1"
    )]
    ClosingInventoryRejected(String),
    #[error(
        "Closing evidence is missing or does not match the stored Closing report; prepare again from a completed v1 Closing"
    )]
    InvalidClosingEvidence,
    #[error("Agent state changed during upgrade preparation: {0}")]
    StateChanged(String),
    #[error("the Closing report is invalid: {0}")]
    InvalidReport(String),
    #[error("the Server preparation baseline could not be read: {0}")]
    ServerUnavailable(String),
    #[error("the Server baseline does not match the Closing declaration: {0}")]
    BaselineMismatch(String),
    #[error("report serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// The Server's accepted baseline, as returned by the authenticated Agent API.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerPreparationBaseline {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub accepted_inventory_revision: i64,
    pub accepted_inventory_sha256: Option<String>,
    pub accepted_inventory_protocol_major: Option<i64>,
    pub active_boot_id: Option<String>,
    pub active_boot_status: String,
    pub previous_boot_id: Option<String>,
    pub close_report_id: Option<String>,
    pub close_report_disposition: Option<String>,
    pub last_report_sequence: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct PreparationState {
    agent_id: Option<String>,
    agent_epoch: i64,
    boot_id: Option<String>,
    report_sequence: i64,
    boot_state: String,
    previous_boot_id: Option<String>,
    pending_transition: Option<String>,
    pending_previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    shutdown_report_id: Option<String>,
}

/// The captured frozen-v1 declaration the Closing actually carried.
#[derive(Debug, Clone)]
struct ClosingEvidence {
    report_id: String,
    report_sequence: u64,
    closed_boot_id: String,
    inventory: NodeInventory,
    content_sha256: String,
}

/// One accepted Closing and the Boot linkage it produced.
#[derive(Debug, Clone)]
struct ClosingResult {
    report_id: String,
    /// The delivery-time disposition when this call sent the Closing; None when
    /// the Closing had already been acknowledged before this run (the Server
    /// baseline then supplies the authoritative disposition).
    disposition: Option<ReceiptDisposition>,
    closed_boot_id: String,
    next_boot_id: String,
}

/// Prepare this frozen-v1 Agent for a coordinated v2 cutover.
///
/// The caller must have stopped normal collection; the exclusive runtime lock
/// enforces that no `run` process owns the Store. On success the Store has a
/// cleanly closed Boot, a new `drained_pending` Boot, all pending Reports
/// acknowledged, and one verified preparation-evidence row.
pub async fn prepare_v1_upgrade(
    config: &AgentConfig,
    sender_deadline: Duration,
) -> Result<PreparationOutcome, PreparationError> {
    // Check the configuration before touching the credential or the Store so a
    // v2 configuration fails with the migration error rather than an unrelated
    // credential problem.
    if config.validated_inventory()?.server_managed_inventory {
        return Err(PreparationError::AlreadyServerManaged);
    }
    let transport = crate::reporting::HttpReportTransport::from_config(config)?;
    prepare_v1_upgrade_with_transport(config, sender_deadline, &transport).await
}

/// Testable core of the public entry point, generic over the Report transport
/// so scenarios can inject a transport that loses a response without weakening
/// the production path.
pub(crate) async fn prepare_v1_upgrade_with_transport<T: crate::reporting::ReportTransport>(
    config: &AgentConfig,
    sender_deadline: Duration,
    transport: &T,
) -> Result<PreparationOutcome, PreparationError> {
    // A Server-managed (v2) configuration has no Agent-assigned revision: this
    // command is only meaningful while the frozen v1 declaration is still in
    // force. It must fail loudly instead of silently preparing nothing.
    if config.validated_inventory()?.server_managed_inventory {
        return Err(PreparationError::AlreadyServerManaged);
    }
    let _runtime_lock = AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| PreparationError::RuntimeOwned(error.to_string()))?;
    let write_permit = AgentStoreWritePermit::new();
    let mut store = AgentStore::open_with_write_permit(
        AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await?;

    let state = load_state(&mut store).await?;
    if state.boot_state == "draining" {
        return Err(PreparationError::RecoveryRequired(state.boot_state));
    }
    let agent_id = state
        .agent_id
        .clone()
        .ok_or(PreparationError::NotEnrolled)?;
    let agent_epoch = state.agent_epoch.max(0) as u64;
    let current_boot_id = state.boot_id.clone().ok_or(PreparationError::NotEnrolled)?;
    let agent_id_parsed = AgentId::from_str(&agent_id)
        .map_err(|error| PreparationError::StateChanged(error.to_string()))?;

    // 1. Determine the final Closing. A finished preparation (or a prior
    //    graceful shutdown) already left the Boot drained_pending; otherwise
    //    resume an un-applied Closing or generate one for the current Boot.
    let closing = if state.boot_state == "drained_pending"
        && state.pending_transition.as_deref() == Some("drained_previous")
    {
        existing_closing(&state)?
    } else {
        complete_final_closing(
            &mut store,
            config,
            &state,
            &current_boot_id,
            sender_deadline,
            transport,
        )
        .await?
    };
    if closing.disposition == Some(ReceiptDisposition::Rejected) {
        return Err(PreparationError::ClosingRejected(closing.report_id));
    }

    // 2. Capture the complete frozen-v1 declaration used by that Closing,
    //    before any canonicalization. Its hash is the original v1 hash: it
    //    includes the revision and preserves the declared Node order.
    let evidence = capture_closing_evidence(
        &mut store,
        &closing.report_id,
        &agent_id_parsed,
        agent_epoch,
        &closing.closed_boot_id,
    )
    .await?;

    // 3. Verify the original hash and revision against the Server's last
    //    accepted values. The local record, the current configuration and the
    //    Server's Node projections cannot substitute for this evidence.
    let baseline = fetch_baseline(config).await?;
    if let Err(error) = verify_baseline(&baseline, &agent_id, agent_epoch, &evidence) {
        // A definitive mismatch invalidates any earlier verified result: it can
        // no longer match the Server, so it must not remain a consumable
        // migratable result. Closing failures and transient read failures never
        // reach this arm and therefore leave existing evidence untouched.
        clear_preparation(&mut store).await?;
        return Err(error);
    }
    // The Server owns the accepted result: bind its recorded disposition rather
    // than an Agent-local derivation, and never accept a rejected Closing.
    let disposition = match baseline_disposition(&baseline) {
        Ok(disposition) => disposition,
        Err(error) => {
            if matches!(
                error,
                PreparationError::BaselineMismatch(_) | PreparationError::ClosingRejected(_)
            ) {
                clear_preparation(&mut store).await?;
            }
            return Err(error);
        }
    };

    // 4. Record the bounded, verified preparation result.
    let verified_at = now_rfc3339();
    // Store the exact frozen-v1 serialization the hash covers (revision plus
    // declared Node order), not just the Node array, so a checkpoint can
    // recompute the hash independently.
    let declaration_json = serde_json::to_string(&evidence.inventory)?;
    write_preparation(
        &mut store,
        PreparedRow {
            agent_id: &agent_id,
            agent_epoch,
            evidence: &evidence,
            disposition,
            next_boot_id: &closing.next_boot_id,
            baseline: &baseline,
            declaration_json: &declaration_json,
            verified_at: &verified_at,
        },
    )
    .await?;
    store.close().await?;

    Ok(PreparationOutcome {
        closing_report_id: evidence.report_id,
        inventory_revision: evidence.inventory.revision,
        inventory_sha256: evidence.content_sha256,
        closing_receipt_disposition: crate::collector::receipt_disposition_name(disposition),
        closed_boot_id: evidence.closed_boot_id,
        next_boot_id: closing.next_boot_id,
        pending_transition: "drained_previous",
        verified_at,
    })
}

/// The Server-recorded acceptance result of the Closing receipt.
fn baseline_disposition(
    baseline: &ServerPreparationBaseline,
) -> Result<ReceiptDisposition, PreparationError> {
    match baseline.close_report_disposition.as_deref() {
        Some("accepted") => Ok(ReceiptDisposition::Accepted),
        Some("partially_accepted") => Ok(ReceiptDisposition::PartiallyAccepted),
        Some("rejected") => Err(PreparationError::ClosingRejected(
            baseline.close_report_id.clone().unwrap_or_default(),
        )),
        Some(other) => Err(PreparationError::BaselineMismatch(format!(
            "Server recorded an unknown Closing disposition {other}"
        ))),
        None => Err(PreparationError::BaselineMismatch(
            "the Server has no recorded disposition for the Closing receipt".to_owned(),
        )),
    }
}

async fn load_state(store: &mut AgentStore) -> Result<PreparationState, PreparationError> {
    sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id, shutdown_report_id FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?
    .ok_or(PreparationError::NotEnrolled)
}

/// Reuse a Closing whose accepted receipt already advanced the Boot. The
/// `drained_pending` transition only exists for a non-rejected Closing, and
/// the Server baseline supplies the authoritative disposition afterwards.
fn existing_closing(state: &PreparationState) -> Result<ClosingResult, PreparationError> {
    let report_id = state
        .close_report_id
        .clone()
        .ok_or(PreparationError::InvalidClosingEvidence)?;
    let closed_boot_id = state
        .pending_previous_boot_id
        .clone()
        .or_else(|| state.previous_boot_id.clone())
        .ok_or(PreparationError::InvalidClosingEvidence)?;
    let next_boot_id = state
        .boot_id
        .clone()
        .ok_or(PreparationError::InvalidClosingEvidence)?;
    Ok(ClosingResult {
        report_id,
        disposition: None,
        closed_boot_id,
        next_boot_id,
    })
}

/// Deliver the existing immutable backlog, oldest-first, before a Closing is
/// generated. A rejected receipt is still an applied receipt, so the loop
/// continues; a transport or sender-deadline failure stops preparation with
/// every unacknowledged Report still in the Spool.
async fn drain_backlog<T: crate::reporting::ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_until: tokio::time::Instant,
) -> Result<(), PreparationError> {
    loop {
        if tokio::time::Instant::now() >= send_until {
            return Err(PreparationError::ClosingDeadline);
        }
        match crate::reporting::deliver_one_with_disposition_and_send_deadline(
            store, transport, send_until,
        )
        .await
        {
            Ok(Some(_)) => continue,
            Ok(None) => return Ok(()),
            Err(crate::reporting::ReportStoreError::DeliveryDeadline) => {
                return Err(PreparationError::ClosingDeadline);
            }
            Err(error) => return Err(PreparationError::Delivery(error)),
        }
    }
}

/// Resume or generate the final Closing and deliver Reports oldest-first until
/// it is acknowledged.
async fn complete_final_closing<T: crate::reporting::ReportTransport>(
    store: &mut AgentStore,
    config: &AgentConfig,
    state: &PreparationState,
    current_boot_id: &str,
    sender_deadline: Duration,
    transport: &T,
) -> Result<ClosingResult, PreparationError> {
    let send_until = tokio::time::Instant::now() + sender_deadline;
    // Never build a second Closing behind an un-applied one: the Server would
    // refuse the duplicate as a stale sequence, and the orphaned first Closing
    // would defeat the drain. Reuse the original immutable bytes instead.
    // Otherwise drain the existing backlog *before* persisting the Closing, so
    // the Closing is unambiguously the newest queued report and an ousted
    // report can never be stranded behind a closed Boot.
    let closing_report_id = match unapplied_closing(store, state).await? {
        Some(report_id) => report_id,
        None => {
            drain_backlog(store, transport, send_until).await?;
            persist_final_closing(store, config, state, sender_deadline).await?
        }
    };
    let (disposition, inventory) = loop {
        if tokio::time::Instant::now() >= send_until {
            return Err(PreparationError::ClosingDeadline);
        }
        let delivered = match crate::reporting::deliver_one_with_disposition_and_send_deadline(
            store, transport, send_until,
        )
        .await
        {
            Ok(value) => value,
            Err(crate::reporting::ReportStoreError::DeliveryDeadline) => {
                return Err(PreparationError::ClosingDeadline);
            }
            Err(error) => return Err(PreparationError::Delivery(error)),
        };
        match delivered {
            Some((report, disposition, inventory)) if report.report_id == closing_report_id => {
                break (disposition, inventory);
            }
            // A backlog Report reached the Server; keep draining. No
            // unacknowledged Report is ever discarded.
            Some(_) => continue,
            // The spool emptied without seeing the Closing: the evidence this
            // command must capture is gone, so refuse rather than guess.
            None => return Err(PreparationError::InvalidClosingEvidence),
        }
    };
    if disposition == ReceiptDisposition::Rejected {
        return Err(PreparationError::ClosingRejected(closing_report_id));
    }
    if !matches!(
        inventory,
        Some(InventoryDisposition::Accepted | InventoryDisposition::Unchanged)
    ) {
        return Err(PreparationError::ClosingInventoryRejected(
            closing_report_id,
        ));
    }
    // An accepted Closing must have advanced the local Boot in the same receipt
    // transaction; anything else means the receipt did not apply to this Boot.
    let after = load_state(store).await?;
    let rotated = after.boot_state == "drained_pending"
        && after.pending_transition.as_deref() == Some("drained_previous")
        && after.close_report_id.as_deref() == Some(closing_report_id.as_str())
        && after.pending_previous_boot_id.as_deref() == Some(current_boot_id)
        && after.previous_boot_id.as_deref() == Some(current_boot_id);
    if !rotated {
        return Err(PreparationError::StateChanged(
            "the Server acknowledged the Closing but the local Boot did not advance".to_owned(),
        ));
    }
    let next_boot_id = after
        .boot_id
        .ok_or(PreparationError::InvalidClosingEvidence)?;
    Ok(ClosingResult {
        report_id: closing_report_id,
        disposition: Some(disposition),
        closed_boot_id: current_boot_id.to_owned(),
        next_boot_id,
    })
}

/// The id of a persisted but not yet acknowledged Closing, if any.
async fn unapplied_closing(
    store: &mut AgentStore,
    state: &PreparationState,
) -> Result<Option<String>, PreparationError> {
    let Some(report_id) = state.shutdown_report_id.as_deref() else {
        return Ok(None);
    };
    Ok(
        sqlx::query_scalar::<_, String>("SELECT report_id FROM reports WHERE report_id = ?")
            .bind(report_id)
            .fetch_optional(store.connection())
            .await?,
    )
}

/// Build and persist the canonical immutable Closing for the current Boot,
/// reusing the shutdown report builder so the wire shape is identical to a
/// graceful shutdown.
async fn persist_final_closing(
    store: &mut AgentStore,
    config: &AgentConfig,
    state: &PreparationState,
    sender_deadline: Duration,
) -> Result<String, PreparationError> {
    let agent_id = AgentId::from_str(
        state
            .agent_id
            .as_deref()
            .ok_or(PreparationError::NotEnrolled)?,
    )
    .map_err(|error| PreparationError::StateChanged(error.to_string()))?;
    let boot_id = BootId::from_str(
        state
            .boot_id
            .as_deref()
            .ok_or(PreparationError::NotEnrolled)?,
    )
    .map_err(|error| PreparationError::StateChanged(error.to_string()))?;
    let inventory = config.validated_inventory()?.inventory;
    let last_good = crate::collector::load_last_report(store).await?;
    let mut report = crate::shutdown::build_shutdown_report(
        config,
        agent_id,
        state.agent_epoch.max(0) as u64,
        boot_id,
        state.report_sequence.max(0) as u64 + 1,
        inventory,
        last_good.as_ref(),
    )
    .map_err(|error| PreparationError::InvalidReport(error.to_string()))?;
    report.block_summaries = crate::block::load_block_summaries(store).await?;
    report.history_gaps = crate::block::load_history_gaps(store).await?;
    report.host.spool = crate::collector::current_spool_diagnostics(store)
        .await
        .map(|value| crate::collector::ok(value, report.generated_at))
        .map_err(PreparationError::Database)?;
    if let Some(spool) = report.host.spool.latest.as_mut() {
        spool.shutdown_state = Some("final_stored".to_owned());
        spool.shutdown_started_at = Some(report.generated_at);
        spool.shutdown_deadline_at = Some(crate::shutdown::started_at_plus(
            report.generated_at,
            sender_deadline,
        ));
        spool.shutdown_forced = Some(false);
    }
    report
        .validate()
        .map_err(|error| PreparationError::InvalidReport(error.to_string()))?;
    let body = serde_json::to_vec(&report)?;
    let generated = report.generated_at.to_string();
    let report_id = report.report_id.to_string();
    crate::reporting::persist_closing_report(
        store,
        &report_id,
        report.agent_epoch,
        &report.boot_id.to_string(),
        report.report_sequence,
        &generated,
        &body,
        state.report_sequence.max(0) as u64,
        &state.boot_state,
    )
    .await?;
    Ok(report_id)
}

/// Capture the complete declaration from the persisted Closing body. The body
/// is the immutable original v1 representation, so re-deriving its content hash
/// reproduces the exact value the Server compared: revision-inclusive and
/// Node-order preserving, before any v2 canonicalization.
async fn capture_closing_evidence(
    store: &mut AgentStore,
    closing_report_id: &str,
    agent_id: &AgentId,
    agent_epoch: u64,
    closed_boot_id: &str,
) -> Result<ClosingEvidence, PreparationError> {
    let body: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT last_report_body FROM agent_state WHERE singleton=1")
            .fetch_one(store.connection())
            .await?;
    let body = body.ok_or(PreparationError::InvalidClosingEvidence)?;
    let report: AgentReport =
        serde_json::from_slice(&body).map_err(|_| PreparationError::InvalidClosingEvidence)?;
    if report.report_id.to_string() != closing_report_id
        || report.boot_id.to_string() != closed_boot_id
        || report.boot_transition != BootTransition::Closing
        || report.agent_id != *agent_id
        || report.agent_epoch != agent_epoch
    {
        return Err(PreparationError::InvalidClosingEvidence);
    }
    let inventory = report.inventory;
    let content_sha256 = inventory.content_sha256().to_string();
    Ok(ClosingEvidence {
        report_id: report.report_id.to_string(),
        report_sequence: report.report_sequence,
        closed_boot_id: closed_boot_id.to_owned(),
        inventory,
        content_sha256,
    })
}

/// Read the Server's accepted baseline over the authenticated Agent API.
async fn fetch_baseline(
    config: &AgentConfig,
) -> Result<ServerPreparationBaseline, PreparationError> {
    let credential = load_credential_file(&config.credential_file)
        .map_err(|error| PreparationError::ServerUnavailable(error.to_string()))?;
    let client = reqwest::Client::builder()
        .user_agent(format!("platpulse-agent/{}", crate::VERSION))
        .timeout(PREPARATION_BASELINE_TIMEOUT)
        .build()
        .map_err(|error| PreparationError::ServerUnavailable(error.to_string()))?;
    let url = format!("{}{AGENT_PREPARATION_PATH}", config.server_url);
    let response = client
        .get(url)
        .bearer_auth(credential)
        .send()
        .await
        .map_err(|error| PreparationError::ServerUnavailable(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(PreparationError::ServerUnavailable(format!(
            "Server returned HTTP {}",
            status.as_u16()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| PreparationError::ServerUnavailable(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        PreparationError::ServerUnavailable(format!("baseline response is invalid: {error}"))
    })
}

/// Compare the Closing declaration against the Server's accepted values.
fn verify_baseline(
    baseline: &ServerPreparationBaseline,
    agent_id: &str,
    agent_epoch: u64,
    evidence: &ClosingEvidence,
) -> Result<(), PreparationError> {
    if baseline.agent_id != agent_id {
        return Err(PreparationError::BaselineMismatch(
            "the Server baseline belongs to another Agent identity".to_owned(),
        ));
    }
    if baseline.agent_epoch != agent_epoch as i64 {
        return Err(PreparationError::BaselineMismatch(format!(
            "Server Agent Epoch {} does not match the Store's {agent_epoch}",
            baseline.agent_epoch
        )));
    }
    if baseline.accepted_inventory_protocol_major != Some(1) {
        return Err(PreparationError::BaselineMismatch(format!(
            "the accepted declaration protocol is {:?}, not frozen v1",
            baseline.accepted_inventory_protocol_major
        )));
    }
    if baseline.accepted_inventory_revision != evidence.inventory.revision as i64 {
        return Err(PreparationError::BaselineMismatch(format!(
            "Server accepted revision {} but the Closing declares {}",
            baseline.accepted_inventory_revision, evidence.inventory.revision
        )));
    }
    match baseline.accepted_inventory_sha256.as_deref() {
        Some(hash) if hash == evidence.content_sha256 => {}
        Some(hash) => {
            return Err(PreparationError::BaselineMismatch(format!(
                "Server accepted hash {hash} but the Closing declares {}",
                evidence.content_sha256
            )));
        }
        None => {
            return Err(PreparationError::BaselineMismatch(
                "the Server has no accepted Inventory for this Agent".to_owned(),
            ));
        }
    }
    if baseline.active_boot_id.as_deref() != Some(evidence.closed_boot_id.as_str()) {
        return Err(PreparationError::BaselineMismatch(format!(
            "Server active Boot {:?} is not the Closing's Boot {}",
            baseline.active_boot_id, evidence.closed_boot_id
        )));
    }
    if baseline.active_boot_status != "closed" {
        return Err(PreparationError::BaselineMismatch(format!(
            "Server active Boot status is {} rather than closed",
            baseline.active_boot_status
        )));
    }
    if baseline.close_report_id.as_deref() != Some(evidence.report_id.as_str()) {
        return Err(PreparationError::BaselineMismatch(format!(
            "Server closed the Boot with report {:?} rather than {}",
            baseline.close_report_id, evidence.report_id
        )));
    }
    if !matches!(
        baseline.close_report_disposition.as_deref(),
        Some("accepted" | "partially_accepted")
    ) {
        return Err(PreparationError::BaselineMismatch(format!(
            "Server Closing disposition is {:?} rather than accepted or partially_accepted",
            baseline.close_report_disposition
        )));
    }
    if baseline.last_report_sequence != Some(evidence.report_sequence as i64) {
        return Err(PreparationError::BaselineMismatch(format!(
            "Server last report sequence is {:?} rather than {}",
            baseline.last_report_sequence, evidence.report_sequence
        )));
    }
    Ok(())
}

/// Remove the bounded evidence row when a completed preparation can no longer
/// be re-verified against the Server. The row is a derived, regenerable result,
/// so clearing it prevents a stale result from being consumed while leaving
/// every pending Report, receipt, and Boot linkage untouched.
async fn clear_preparation(store: &mut AgentStore) -> Result<(), PreparationError> {
    let _write_permit = store.acquire_write().await;
    sqlx::query("DELETE FROM upgrade_preparation WHERE singleton=1")
        .execute(store.connection())
        .await?;
    Ok(())
}

struct PreparedRow<'a> {
    agent_id: &'a str,
    agent_epoch: u64,
    evidence: &'a ClosingEvidence,
    disposition: ReceiptDisposition,
    next_boot_id: &'a str,
    baseline: &'a ServerPreparationBaseline,
    declaration_json: &'a str,
    verified_at: &'a str,
}

/// Persist the bounded evidence row in one transaction.
async fn write_preparation(
    store: &mut AgentStore,
    row: PreparedRow<'_>,
) -> Result<(), PreparationError> {
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    sqlx::query(
        "INSERT INTO upgrade_preparation (singleton, agent_id, agent_epoch, closing_report_id, closing_report_sequence, closing_receipt_disposition, inventory_revision, inventory_sha256, declaration_json, closed_boot_id, next_boot_id, pending_transition, accepted_inventory_revision, accepted_inventory_sha256, accepted_inventory_protocol_major, server_active_boot_id, server_active_boot_status, server_close_report_id, verified_at) VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'drained_previous', ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET agent_id=excluded.agent_id, agent_epoch=excluded.agent_epoch, closing_report_id=excluded.closing_report_id, closing_report_sequence=excluded.closing_report_sequence, closing_receipt_disposition=excluded.closing_receipt_disposition, inventory_revision=excluded.inventory_revision, inventory_sha256=excluded.inventory_sha256, declaration_json=excluded.declaration_json, closed_boot_id=excluded.closed_boot_id, next_boot_id=excluded.next_boot_id, pending_transition=excluded.pending_transition, accepted_inventory_revision=excluded.accepted_inventory_revision, accepted_inventory_sha256=excluded.accepted_inventory_sha256, accepted_inventory_protocol_major=excluded.accepted_inventory_protocol_major, server_active_boot_id=excluded.server_active_boot_id, server_active_boot_status=excluded.server_active_boot_status, server_close_report_id=excluded.server_close_report_id, verified_at=excluded.verified_at",
    )
    .bind(row.agent_id)
    .bind(row.agent_epoch as i64)
    .bind(&row.evidence.report_id)
    .bind(row.evidence.report_sequence as i64)
    .bind(crate::collector::receipt_disposition_name(row.disposition))
    .bind(row.evidence.inventory.revision as i64)
    .bind(&row.evidence.content_sha256)
    .bind(row.declaration_json)
    .bind(&row.evidence.closed_boot_id)
    .bind(row.next_boot_id)
    .bind(row.baseline.accepted_inventory_revision)
    .bind(row.baseline.accepted_inventory_sha256.as_deref())
    .bind(row.baseline.accepted_inventory_protocol_major)
    .bind(row.baseline.active_boot_id.as_deref())
    .bind(&row.baseline.active_boot_status)
    .bind(row.baseline.close_report_id.as_deref())
    .bind(row.verified_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

    use platpulse_core::inventory::InventoryNode;
    use platpulse_server::auth::{AuthConfig, create_owner, hash_password};
    use platpulse_server::database::{ServerDatabaseConfig, initialize};
    use platpulse_server::enrollment::{
        ENROLLMENT_TOKEN_DEFAULT_LIFETIME, create_enrollment_token,
    };
    use platpulse_server::http::{AppState, build_app};
    use platpulse_server::network::create_network;
    use platpulse_server::secrets::{create_pepper_file, load_pepper_file};
    use tempfile::TempDir;
    use tokio::task::JoinHandle;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::database::{AgentDatabaseConfig, AgentStore};
    use crate::reporting::{
        HttpReportTransport, ReportStoreError, ReportTransport, deliver_one,
        persist_immutable_report,
    };

    const BOOT_ID: &str = "0195f2a1-0012-4012-8012-000000000012";
    const NODE_ID: &str = "0195f2a1-0014-4014-8014-000000000014";
    const AGENT_EPOCH: i64 = 1;
    const GENERATED_AT: &str = "2026-08-12T09:00:00Z";

    fn report_id(sequence: u64) -> String {
        format!("0195f2a1-0100-4000-8000-0000000000{sequence:02}")
    }

    fn report_body(sequence: u64, agent_id: &str) -> Vec<u8> {
        let mut report: serde_json::Value = serde_json::from_str(include_str!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report["agent_id"] = serde_json::Value::String(agent_id.to_owned());
        report["report_sequence"] = serde_json::json!(sequence);
        report["report_id"] = serde_json::Value::String(report_id(sequence));
        report["generated_at"] = serde_json::json!(GENERATED_AT);
        serde_json::to_vec(&report).unwrap()
    }

    struct RunningServer {
        state: AppState,
        shutdown: CancellationToken,
        task: JoinHandle<()>,
    }

    impl RunningServer {
        async fn stop(self) {
            self.shutdown.cancel();
            let _ = self.task.await;
            self.state.db().close().await;
        }
    }

    async fn boot_server(addr: std::net::SocketAddr, dir: &Path) -> (RunningServer, String) {
        let db = initialize(ServerDatabaseConfig::new(dir.join("server.db")))
            .await
            .unwrap();
        let pepper_path = dir.join("server-pepper");
        create_pepper_file(&pepper_path).unwrap();
        let pepper = load_pepper_file(&pepper_path).unwrap();
        let auth = AuthConfig::development(pepper, format!("http://{addr}"));
        let token = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap()
            .token;
        create_owner(
            &db,
            "admin",
            &hash_password(b"correct horse battery").unwrap(),
        )
        .await
        .unwrap();
        create_network(
            &db,
            "platon-mainnet",
            "PlatON Mainnet",
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            210425,
            210425,
            "lat",
        )
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let state = AppState::new(db, None, auth);
        let shutdown = CancellationToken::new();
        let served = state.clone();
        let stopping = shutdown.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, build_app(served))
                .with_graceful_shutdown(async move { stopping.cancelled().await })
                .await
                .unwrap();
        });
        (
            RunningServer {
                state,
                shutdown,
                task,
            },
            token,
        )
    }

    fn write_agent_config(
        dir: &Path,
        server_url: &str,
        revision: Option<u64>,
        rpc_endpoint: &str,
    ) -> PathBuf {
        let path = dir.join("agent.toml");
        let revision_line = revision
            .map(|revision| format!("inventory_revision={revision}\n"))
            .unwrap_or_default();
        let text = format!(
            "server_url=\"{server_url}\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\n{revision_line}nodes=[{{node_id=\"{NODE_ID}\",network_key=\"platon-mainnet\",rpc_endpoint=\"{rpc_endpoint}\"}}]\n",
            dir.join("credential").display(),
            dir.join("agent.db").display(),
        );
        std::fs::write(&path, text).unwrap();
        path
    }

    async fn open_store(config: &AgentConfig) -> AgentStore {
        AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap()
    }

    async fn seed_boot(config: &AgentConfig, boot_id: &str) {
        let mut store = open_store(config).await;
        sqlx::query("UPDATE agent_state SET boot_id=?, boot_state='active' WHERE singleton=1")
            .bind(boot_id)
            .execute(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();
    }

    async fn seed_report_sequence(config: &AgentConfig, sequence: i64) {
        let mut store = open_store(config).await;
        sqlx::query("UPDATE agent_state SET report_sequence=? WHERE singleton=1")
            .bind(sequence)
            .execute(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();
    }

    async fn queued_reports(config: &AgentConfig) -> i64 {
        let mut store = open_store(config).await;
        let count = sqlx::query_scalar("SELECT COUNT(*) FROM reports")
            .fetch_one(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();
        count
    }

    async fn shutdown_report_id(config: &AgentConfig) -> Option<String> {
        let mut store = open_store(config).await;
        let value: Option<String> =
            sqlx::query_scalar("SELECT shutdown_report_id FROM agent_state WHERE singleton=1")
                .fetch_one(store.connection())
                .await
                .unwrap();
        store.close().await.unwrap();
        value
    }

    #[derive(sqlx::FromRow)]
    struct BootSnapshot {
        boot_state: String,
        boot_id: Option<String>,
        previous_boot_id: Option<String>,
        pending_transition: Option<String>,
        pending_previous_boot_id: Option<String>,
        close_report_id: Option<String>,
        report_sequence: i64,
    }

    async fn boot_snapshot(config: &AgentConfig) -> BootSnapshot {
        let mut store = open_store(config).await;
        let row = sqlx::query_as::<_, BootSnapshot>(
            "SELECT boot_state, boot_id, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id, report_sequence FROM agent_state WHERE singleton=1",
        )
        .fetch_one(store.connection())
        .await
        .unwrap();
        store.close().await.unwrap();
        row
    }

    #[derive(sqlx::FromRow)]
    struct EvidenceRow {
        closing_report_id: String,
        inventory_revision: i64,
        inventory_sha256: String,
        disposition: String,
        accepted_inventory_revision: i64,
        accepted_inventory_sha256: Option<String>,
        declaration_json: String,
        closed_boot_id: String,
        next_boot_id: String,
    }

    async fn evidence_row(config: &AgentConfig) -> Option<EvidenceRow> {
        let mut store = open_store(config).await;
        let row = sqlx::query_as::<_, EvidenceRow>(
            "SELECT closing_report_id, inventory_revision, inventory_sha256, closing_receipt_disposition AS disposition, accepted_inventory_revision, accepted_inventory_sha256, declaration_json, closed_boot_id, next_boot_id FROM upgrade_preparation WHERE singleton=1",
        )
        .fetch_optional(store.connection())
        .await
        .unwrap();
        store.close().await.unwrap();
        row
    }

    async fn prepare(config: &AgentConfig) -> Result<PreparationOutcome, PreparationError> {
        let transport = HttpReportTransport::from_config(config).unwrap();
        prepare_v1_upgrade_with_transport(config, Duration::from_secs(5), &transport).await
    }

    /// Simulates a Server that committed a report but whose response never made
    /// it back to the Agent: the Closing must be resumed with identical bytes.
    struct DropClosingResponseTransport {
        inner: HttpReportTransport,
        dropped: AtomicBool,
    }

    impl ReportTransport for DropClosingResponseTransport {
        fn send<'a>(
            &'a self,
            body: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ReportStoreError>> + Send + 'a>> {
            Box::pin(async move {
                let response = self.inner.send(body).await?;
                let is_closing = serde_json::from_slice::<serde_json::Value>(body)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("boot_transition")
                            .and_then(|v| v.as_str())
                            .map(|v| v == "closing")
                    })
                    .unwrap_or(false);
                if is_closing && !self.dropped.swap(true, Ordering::SeqCst) {
                    return Err(ReportStoreError::Delivery(
                        "simulated lost receipt".to_owned(),
                    ));
                }
                Ok(response)
            })
        }
    }

    #[tokio::test]
    async fn preparation_drains_the_backlog_and_closes_with_verified_evidence() {
        let dir = TempDir::new().unwrap();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let (server, token) = boot_server(addr, dir.path()).await;
        let path = write_agent_config(
            dir.path(),
            &format!("http://{addr}"),
            Some(1),
            "ws://127.0.0.1:6790",
        );
        let config = AgentConfig::resolve(&path).unwrap();
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();
        seed_boot(&config, BOOT_ID).await;

        let mut store = open_store(&config).await;
        for sequence in 1..=3 {
            persist_immutable_report(
                &mut store,
                &report_id(sequence),
                AGENT_EPOCH as u64,
                BOOT_ID,
                sequence,
                GENERATED_AT,
                &report_body(sequence, &agent_id),
            )
            .await
            .unwrap();
        }
        store.close().await.unwrap();
        seed_report_sequence(&config, 3).await;

        let outcome = prepare(&config).await.unwrap();
        let expected_hash = config
            .validated_inventory()
            .unwrap()
            .inventory
            .content_sha256()
            .to_string();
        assert_eq!(outcome.inventory_revision, 1);
        assert_eq!(outcome.inventory_sha256, expected_hash);
        assert_eq!(outcome.closed_boot_id, BOOT_ID);
        assert_ne!(outcome.next_boot_id, BOOT_ID);
        assert_eq!(outcome.pending_transition, "drained_previous");

        assert_eq!(queued_reports(&config).await, 0);
        let snapshot = boot_snapshot(&config).await;
        assert_eq!(snapshot.boot_state, "drained_pending");
        assert_ne!(snapshot.boot_id.as_deref(), Some(BOOT_ID));
        assert_eq!(snapshot.previous_boot_id.as_deref(), Some(BOOT_ID));
        assert_eq!(
            snapshot.pending_transition.as_deref(),
            Some("drained_previous")
        );
        assert_eq!(snapshot.pending_previous_boot_id.as_deref(), Some(BOOT_ID));
        assert_eq!(
            snapshot.close_report_id.as_deref(),
            Some(outcome.closing_report_id.as_str())
        );
        assert_eq!(snapshot.report_sequence, 0);

        let evidence = evidence_row(&config).await.expect("verified evidence");
        assert_eq!(evidence.closing_report_id, outcome.closing_report_id);
        assert_eq!(evidence.inventory_revision, 1);
        assert_eq!(evidence.inventory_sha256, expected_hash);
        assert_eq!(evidence.accepted_inventory_revision, 1);
        assert_eq!(
            evidence.accepted_inventory_sha256.as_deref(),
            Some(expected_hash.as_str())
        );
        assert_eq!(evidence.closed_boot_id, BOOT_ID);
        assert_eq!(evidence.next_boot_id, outcome.next_boot_id);
        assert!(evidence.declaration_json.contains(NODE_ID));
        // The stored declaration is exactly what the hash covers, so a later
        // checkpoint can recompute it.
        let declared: NodeInventory = serde_json::from_str(&evidence.declaration_json).unwrap();
        assert_eq!(
            declared.content_sha256().to_string(),
            evidence.inventory_sha256
        );
        assert!(matches!(
            evidence.disposition.as_str(),
            "accepted" | "partially_accepted"
        ));

        // Re-running is a safe, idempotent retry: it reuses the same Closing and
        // never invents a second one.
        let again = prepare(&config).await.unwrap();
        assert_eq!(again.closing_report_id, outcome.closing_report_id);
        assert_eq!(again.closed_boot_id, BOOT_ID);
        server.stop().await;
    }

    #[tokio::test]
    async fn preparation_resumes_a_closing_whose_receipt_was_lost() {
        let dir = TempDir::new().unwrap();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let (server, token) = boot_server(addr, dir.path()).await;
        let path = write_agent_config(
            dir.path(),
            &format!("http://{addr}"),
            Some(1),
            "ws://127.0.0.1:6790",
        );
        let config = AgentConfig::resolve(&path).unwrap();
        crate::enroll::enroll_agent(&config, &token).await.unwrap();
        seed_boot(&config, BOOT_ID).await;

        let transport = DropClosingResponseTransport {
            inner: HttpReportTransport::from_config(&config).unwrap(),
            dropped: AtomicBool::new(false),
        };
        let first =
            prepare_v1_upgrade_with_transport(&config, Duration::from_secs(5), &transport).await;
        assert!(
            matches!(first, Err(PreparationError::Delivery(_))),
            "unexpected first result: {first:?}"
        );

        let persisted = shutdown_report_id(&config)
            .await
            .expect("Closing persisted");
        assert_eq!(queued_reports(&config).await, 1);
        assert_eq!(boot_snapshot(&config).await.boot_state, "active");

        let outcome = prepare(&config).await.unwrap();
        assert_eq!(outcome.closing_report_id, persisted);
        assert_eq!(outcome.closed_boot_id, BOOT_ID);
        assert_eq!(queued_reports(&config).await, 0);
        assert_eq!(boot_snapshot(&config).await.boot_state, "drained_pending");
        server.stop().await;
    }

    #[tokio::test]
    async fn preparation_closes_an_empty_queue_boot() {
        let dir = TempDir::new().unwrap();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let (server, token) = boot_server(addr, dir.path()).await;
        let path = write_agent_config(
            dir.path(),
            &format!("http://{addr}"),
            Some(1),
            "ws://127.0.0.1:6790",
        );
        let config = AgentConfig::resolve(&path).unwrap();
        crate::enroll::enroll_agent(&config, &token).await.unwrap();
        seed_boot(&config, BOOT_ID).await;

        // Empty Spool alone is not the gate: the Boot is still open and only a
        // completed Closing may advance it.
        assert_eq!(queued_reports(&config).await, 0);
        let outcome = prepare(&config).await.unwrap();
        assert_eq!(outcome.closed_boot_id, BOOT_ID);
        assert_eq!(boot_snapshot(&config).await.boot_state, "drained_pending");
        assert!(evidence_row(&config).await.is_some());
        server.stop().await;
    }

    #[tokio::test]
    async fn preparation_stops_on_a_rejected_closing() {
        let dir = TempDir::new().unwrap();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let (server, token) = boot_server(addr, dir.path()).await;
        let path = write_agent_config(
            dir.path(),
            &format!("http://{addr}"),
            Some(1),
            "ws://127.0.0.1:6790",
        );
        let config = AgentConfig::resolve(&path).unwrap();
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();
        seed_boot(&config, BOOT_ID).await;

        // Establish an accepted revision 1 with content A.
        let mut store = open_store(&config).await;
        persist_immutable_report(
            &mut store,
            &report_id(1),
            AGENT_EPOCH as u64,
            BOOT_ID,
            1,
            GENERATED_AT,
            &report_body(1, &agent_id),
        )
        .await
        .unwrap();
        let transport = HttpReportTransport::from_config(&config).unwrap();
        deliver_one(&mut store, &transport).await.unwrap();
        store.close().await.unwrap();
        seed_report_sequence(&config, 1).await;

        // Drift the declaration at the same revision: the Server must refuse the
        // Closing, and preparation must stay in v1.
        write_agent_config(
            dir.path(),
            &format!("http://{addr}"),
            Some(1),
            "ws://127.0.0.1:6791",
        );
        let result = prepare(&config).await;
        assert!(
            matches!(result, Err(PreparationError::ClosingRejected(_))),
            "unexpected result: {result:?}"
        );
        assert_eq!(boot_snapshot(&config).await.boot_state, "active");
        assert!(evidence_row(&config).await.is_none());
        server.stop().await;
    }

    #[tokio::test]
    async fn preparation_refuses_a_modified_server_baseline() {
        let dir = TempDir::new().unwrap();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let (server, token) = boot_server(addr, dir.path()).await;
        let path = write_agent_config(
            dir.path(),
            &format!("http://{addr}"),
            Some(1),
            "ws://127.0.0.1:6790",
        );
        let config = AgentConfig::resolve(&path).unwrap();
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();
        seed_boot(&config, BOOT_ID).await;

        prepare(&config).await.unwrap();

        // The Server's accepted declaration changed after the Closing was
        // accepted. The original Closing bytes no longer prove the accepted
        // baseline, so preparation must refuse rather than invent a v2 baseline.
        sqlx::query("UPDATE agents SET inventory_sha256='0xdeadbeef' WHERE agent_id=?")
            .bind(&agent_id)
            .execute(server.state.db().pool())
            .await
            .unwrap();
        let result = prepare(&config).await;
        match result {
            Err(PreparationError::BaselineMismatch(message)) => {
                assert!(
                    message.contains("0xdeadbeef"),
                    "unexpected message: {message}"
                )
            }
            other => panic!("unexpected result: {other:?}"),
        }
        // A definitive mismatch clears the earlier verified result so no stale
        // migratable result can be consumed, while the completed Boot linkage
        // and the Closing itself are left intact.
        assert!(evidence_row(&config).await.is_none());
        let snapshot = boot_snapshot(&config).await;
        assert_eq!(snapshot.boot_state, "drained_pending");
        assert!(snapshot.close_report_id.is_some());
        server.stop().await;
    }

    #[tokio::test]
    async fn preparation_refuses_a_server_managed_configuration() {
        let dir = TempDir::new().unwrap();
        let path = write_agent_config(
            dir.path(),
            "http://127.0.0.1:9",
            None,
            "ws://127.0.0.1:6790",
        );
        let config = AgentConfig::resolve(&path).unwrap();
        assert!(matches!(
            prepare_v1_upgrade(&config, Duration::from_secs(1)).await,
            Err(PreparationError::AlreadyServerManaged)
        ));
    }

    #[test]
    fn baseline_verification_refuses_each_mismatch() {
        let inventory = NodeInventory {
            revision: 4,
            nodes: vec![InventoryNode {
                node_id: NODE_ID.parse().unwrap(),
                display_name: None,
                network_key: "platon-mainnet".parse().unwrap(),
                rpc_endpoint: "ws://127.0.0.1:6790".parse().unwrap(),
                process: None,
            }],
        };
        let evidence = ClosingEvidence {
            report_id: "0195f2a1-0130-4013-8013-000000000013".to_owned(),
            report_sequence: 9,
            closed_boot_id: BOOT_ID.to_owned(),
            content_sha256: inventory.content_sha256().to_string(),
            inventory,
        };
        let baseline = ServerPreparationBaseline {
            agent_id: "0195f2a1-0011-4011-8011-000000000011".to_owned(),
            agent_epoch: AGENT_EPOCH,
            accepted_inventory_revision: 4,
            accepted_inventory_sha256: Some(evidence.content_sha256.clone()),
            accepted_inventory_protocol_major: Some(1),
            active_boot_id: Some(BOOT_ID.to_owned()),
            active_boot_status: "closed".to_owned(),
            previous_boot_id: None,
            close_report_id: Some(evidence.report_id.clone()),
            close_report_disposition: Some("accepted".to_owned()),
            last_report_sequence: Some(9),
        };
        assert!(
            verify_baseline(&baseline, &baseline.agent_id, AGENT_EPOCH as u64, &evidence).is_ok()
        );

        let mut changed = baseline.clone();
        changed.agent_id = "0195f2a1-00ff-40ff-80ff-0000000000ff".to_owned();
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));

        let mut changed = baseline.clone();
        changed.accepted_inventory_protocol_major = Some(2);
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));

        let mut changed = baseline.clone();
        changed.accepted_inventory_revision = 3;
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));

        let mut changed = baseline.clone();
        changed.accepted_inventory_sha256 = Some("0xother".to_owned());
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));

        let mut changed = baseline.clone();
        changed.active_boot_status = "active".to_owned();
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));

        let mut changed = baseline.clone();
        changed.close_report_id = Some("0195f2a1-0130-4013-8013-0000000000ff".to_owned());
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));

        let mut changed = baseline.clone();
        changed.last_report_sequence = Some(8);
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));

        let mut changed = baseline.clone();
        changed.close_report_disposition = Some("rejected".to_owned());
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));

        let mut changed = baseline.clone();
        changed.close_report_disposition = None;
        assert!(matches!(
            verify_baseline(&changed, &baseline.agent_id, AGENT_EPOCH as u64, &evidence),
            Err(PreparationError::BaselineMismatch(_))
        ));
    }

    /// The operator-facing entry point, not just the library seam: it resolves
    /// the configuration itself and completes the preparation.
    #[tokio::test]
    async fn preparation_cli_entry_completes_and_verifies() {
        let dir = TempDir::new().unwrap();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let (server, token) = boot_server(addr, dir.path()).await;
        let path = write_agent_config(
            dir.path(),
            &format!("http://{addr}"),
            Some(1),
            "ws://127.0.0.1:6790",
        );
        let config = AgentConfig::resolve(&path).unwrap();
        crate::enroll::enroll_agent(&config, &token).await.unwrap();
        seed_boot(&config, BOOT_ID).await;

        let args = crate::cli::PrepareUpgradeArgs {
            config: path.clone(),
            deadline_ms: 5_000,
        };
        crate::cli::run_prepare_upgrade(&args).await.unwrap();

        assert_eq!(boot_snapshot(&config).await.boot_state, "drained_pending");
        let evidence = evidence_row(&config).await.expect("verified evidence");
        assert_eq!(evidence.closed_boot_id, BOOT_ID);
        assert_eq!(evidence.inventory_revision, 1);
        server.stop().await;
    }
}
