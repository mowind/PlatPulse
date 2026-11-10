//! The report handlers and the transactional ingestion of one AgentReport.
use crate::http::realtime::{AdminResource, PublicResource};
use std::collections::HashSet;
use std::str::FromStr;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use sha2::{Digest, Sha256};
use sqlx::FromRow;

use crate::enrollment::AgentAuthInfo;
use crate::http::{AppState, RequestId};
use platpulse_core::component::ComponentKey;
use platpulse_core::protocol::SUPPORTED_PROTOCOL_MAJORS;
use platpulse_core::{
    AgentReport, ComponentRevision, InventoryAcceptance, InventoryDeclaration,
    InventoryDisposition, InventoryReceiptV2, NodeCurrentDisposition, NodeReceipt,
    ReceiptDisposition, Rejection, ReportInventory, ReportReceipt, ReportReceiptV2,
    SampleDisposition, SampleDispositionKind, SampleRef, Sha256Hex,
};

use super::*;
use crate::block_history::{NodeAdmission, Refusal, admit, observe_gap_settlement};

#[derive(Debug, FromRow)]
struct AgentRow {
    agent_epoch: i64,
    active_boot_id: Option<String>,
    active_boot_status: String,
    last_report_sequence: Option<i64>,
    last_inventory_revision: i64,
}

/// The Receipt line for one sample: terminal exactly when admission refused it.
fn disposition(
    node_id: platpulse_core::NodeId,
    sample: SampleRef,
    rejection: Option<Rejection>,
) -> SampleDisposition {
    SampleDisposition {
        node_id,
        sample,
        disposition: if rejection.is_some() {
            SampleDispositionKind::TerminalRejected
        } else {
            SampleDispositionKind::Accepted
        },
        rejection,
    }
}

pub(crate) fn validate_report_body_size(len: usize) -> Result<(), StatusCode> {
    if len > platpulse_core::protocol::MAX_REPORT_BODY_BYTES {
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    } else {
        Ok(())
    }
}

pub(super) async fn handler(
    State(state): State<AppState>,
    Extension(auth): Extension<AgentAuthInfo>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    let parsed: AgentReport = match serde_json::from_slice(&body) {
        Ok(report) => report,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_report",
                "Agent report is invalid",
            );
        }
    };
    ingest_report(state, auth, request_id, body, parsed).await
}

/// v2 ingestion entry point: the report declares Inventory content only and the
/// Server allocates the accepted revision and canonical fingerprint.
pub(super) async fn handler_v2(
    State(state): State<AppState>,
    Extension(auth): Extension<AgentAuthInfo>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    let parsed: AgentReport<InventoryDeclaration> = match serde_json::from_slice(&body) {
        Ok(report) => report,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_report",
                "Agent report is invalid",
            );
        }
    };
    ingest_report(state, auth, request_id, body, parsed).await
}

async fn ingest_report<I: ReportInventory>(
    state: AppState,
    auth: AgentAuthInfo,
    request_id: RequestId,
    body: Bytes,
    parsed: AgentReport<I>,
) -> Response {
    // Only the authenticated Agent identity is trusted before validation; the
    // report identity is attached once the report validated and matched it.
    let span = tracing::Span::current();
    span.record("agent_id", auth.agent_id.as_str());
    if state.is_shutting_down() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting_down",
            "Server is shutting down",
        );
    }
    let _ingestion = match state.ingestion_guard() {
        Some(guard) => guard,
        None => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "shutting_down",
                "Server is shutting down",
            );
        }
    };
    if validate_report_body_size(body.len()).is_err() {
        return error(
            &request_id.0,
            StatusCode::PAYLOAD_TOO_LARGE,
            "report_too_large",
            "Agent report exceeds the protocol size limit",
        );
    }
    let digest = Sha256::digest(&body);
    let hash = Sha256Hex::from_str(&format!("0x{digest:x}")).expect("SHA-256 output is valid");
    if parsed.validate().is_err() {
        return error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_report",
            "Agent report is invalid",
        );
    }
    if parsed.agent_id.to_string() != auth.agent_id {
        return error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "agent_identity_mismatch",
            "Agent identity is not authorized",
        );
    }
    span.record("boot_id", tracing::field::display(&parsed.boot_id));
    span.record("report_id", tracing::field::display(&parsed.report_id));
    span.record("report_sequence", parsed.report_sequence);
    tracing::debug!(
        "report received with {} Node observation(s)",
        parsed.nodes.len()
    );
    // Issue #192: a converted deployment stays gated until the explicit cutover
    // resume. No collection/ingestion, Admin mutation or external-effect worker
    // is authorized before then, so a converted database can never be served as
    // if the switch had happened.
    let cutover = state.db().inventory_cutover();
    if cutover.business_writes_blocked() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "inventory_cutover_required",
            "Converted deployment awaits the coordinated Inventory cutover; stop the Server and run 'platpulse-server cutover resume'",
        );
    }
    // Acceptance below reads this Agent's active Boot, last accepted sequence
    // and Inventory revision and then writes the next state from them, which is
    // a read-then-write invariant two concurrent Reports of one Agent would
    // break (ADR 0007, ADR 0013): the newer Report could commit first and the
    // older one afterwards, moving the sequence and the Current Projection
    // backwards. The fence is taken before the transaction so a waiting Report
    // holds no connection, and it is per Agent, so Reports of different Agents
    // still ingest in parallel. SQLite's single write connection made this hold
    // already; the PostgreSQL work connections do not.
    let fences = state.write_fences();
    let _agent_fence = fences.agents.hold(&auth.agent_id).await;
    // Node admission is a read-then-write invariant across Agents and the
    // Owner (issue #280): the receipt reads a Node's purge tombstone and owner
    // and then upserts its projection. Two Agents first declaring one Node, or
    // a Purge or Transfer committing between that read and the upsert, would
    // otherwise let a Report take over another Agent's Node or rebuild a
    // purged one. Every Node this Report names is held, in ascending order,
    // before any connection is taken.
    let _node_fences = fences
        .nodes
        .hold_all(
            parsed
                .inventory
                .nodes()
                .iter()
                .map(|node| node.node_id.to_string())
                .chain(parsed.nodes.iter().map(|node| node.node_id.to_string()))
                .chain(
                    parsed
                        .block_summaries
                        .iter()
                        .map(|sample| sample.node_id.to_string()),
                )
                .chain(
                    parsed
                        .history_gaps
                        .iter()
                        .map(|gap| gap.node_id.to_string()),
                ),
        )
        .await;
    // Hold the retention sweep off this receipt: retention and ingestion write
    // the same series-state rows, and on PostgreSQL the deadlock that follows
    // may abort the Report. Hygiene must never be able to refuse a Report, so
    // the sweep waits for the receipts in flight instead.
    let _sweep_off = crate::retention::hold_sweep_off().await;
    let mut tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(db_error) => {
            state.note_database_error(&db_error);
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };

    match find_replay::<I>(&mut tx, &auth.agent_id, parsed.report_id, &hash).await {
        Ok(None) => {}
        Ok(Some(replay)) => return replay.into_response(&request_id.0),
        Err(failure) => {
            return storage_failure(&state, &request_id.0, "read stored receipt", &failure);
        }
    }
    // After the coordinated cutover the frozen v1 route is replay-only. An
    // unseen v1 Report is explicitly unsupported: it creates no acceptance
    // Receipt, advances no Boot, allocates no revision and changes no
    // projection. It is not mixed-version ingestion, an automatic downgrade or
    // an old-backlog submission path (design 15.10.5).
    if cutover.v1_replay_only()
        && I::protocol_version() != platpulse_core::protocol::PROTOCOL_VERSION_V2
    {
        return error(
            &request_id.0,
            StatusCode::CONFLICT,
            "report_not_replayable",
            "Frozen v1 Reports are replay-only after the Inventory cutover; new Reports must use protocol v2",
        );
    }
    // A removed Agent carries the durable removal boundary (design §15.2,
    // #171). Reading the row inside the receipt transaction with the live
    // predicate makes a late report fail exactly like an invalid credential,
    // so no later Inventory, retry, or restart can reconstruct its Nodes.
    let agent = match sqlx::query_as::<_, AgentRow>(
        "SELECT agent_epoch, active_boot_id, active_boot_status, last_report_sequence, last_inventory_revision FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
    )
    .bind(&auth.agent_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error(
                &request_id.0,
                StatusCode::UNAUTHORIZED,
                "agent_auth_required",
                "Agent credential is invalid",
            );
        }
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if parsed.agent_epoch != agent.agent_epoch as u64 {
        return store_rejected(
            &state,
            tx,
            &parsed,
            hash.clone(),
            rejected(
                parsed.report_id,
                hash,
                platpulse_core::RejectionCode::StaleBoot,
                "Agent epoch is stale",
            ),
            &request_id.0,
        )
        .await;
    }

    // A failed read must not pass for "this boot was never seen": that would
    // skip the closed-boot refusal below.
    let old_boot_status: Option<String> = match sqlx::query_scalar(
        "SELECT status FROM agent_boots WHERE agent_id=$1 AND agent_epoch=$2 AND boot_id=$3",
    )
    .bind(&auth.agent_id)
    .bind(parsed.agent_epoch as i64)
    .bind(parsed.boot_id.to_string())
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(status) => status,
        Err(failure) => return storage_error(&request_id.0, "read boot status", &failure),
    };
    let declared_revision = parsed.inventory.declared_revision();
    // Frozen v1: the Agent declares its own monotonically increasing revision.
    if let Some(declared) = declared_revision {
        if declared < agent.last_inventory_revision as u64 {
            return store_rejected(
                &state,
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::InventoryRevisionConflict,
                    "Inventory revision is older than the accepted revision",
                ),
                &request_id.0,
            )
            .await;
        }
    }

    let inventory_hash = parsed.inventory.accepted_sha256();
    // An Agent without an accepted Inventory stores NULL: no prior hash.
    let prior_inventory_hash: Option<String> = match sqlx::query_scalar::<_, Option<String>>(
        "SELECT inventory_sha256 FROM agents WHERE agent_id=$1",
    )
    .bind(&auth.agent_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(hash) => hash.flatten(),
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if let Some(declared) = declared_revision {
        if declared == agent.last_inventory_revision as u64
            && prior_inventory_hash.is_some()
            && prior_inventory_hash.as_deref() != Some(inventory_hash.as_str())
        {
            return store_rejected(
                &state,
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::InventoryRevisionConflict,
                    "Inventory content conflicts at the accepted revision",
                ),
                &request_id.0,
            )
            .await;
        }
    }

    // v2 declares content only: the Server allocates the accepted revision inside
    // this authoritative transaction. Identical content keeps the accepted
    // revision; any content change (including A -> B -> A) advances it. The
    // comparison and allocation commit with the receipt, so concurrent reports
    // can never allocate the same revision twice.
    let inventory_unchanged = prior_inventory_hash.as_deref() == Some(inventory_hash.as_str());
    let effective_inventory_revision: u64 = match declared_revision {
        Some(declared) => declared,
        None if inventory_unchanged => agent.last_inventory_revision.max(0) as u64,
        None => {
            // The column is a positive signed 64-bit integer, so the ceiling is
            // i64::MAX even though the arithmetic runs in u64. The checked add
            // plus the signed-range filter keep exhaustion an explicit refusal
            // instead of a cast that would wrap negative.
            let last = agent.last_inventory_revision.max(0) as u64;
            let Some(next) = last.checked_add(1).filter(|next| *next <= i64::MAX as u64) else {
                return store_rejected(
                    &state,
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::InventoryRevisionConflict,
                        "Inventory revision range is exhausted",
                    ),
                    &request_id.0,
                )
                .await;
            };
            next
        }
    };

    // A sequence is unique within one boot. Never silently accept a competing
    // body. This early read spares a doomed Report its projection work;
    // store_receipt is the barrier that holds under concurrency.
    let sequence_conflict = match sqlx::query_scalar::<_, String>(
        "SELECT report_id FROM agent_report_receipts WHERE agent_id = $1 AND agent_epoch = $2 AND boot_id = $3 AND report_sequence = $4",
    )
    .bind(&auth.agent_id)
    .bind(parsed.agent_epoch as i64)
    .bind(parsed.boot_id.to_string())
    .bind(parsed.report_sequence as i64)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(taken) => taken,
        Err(failure) => {
            return storage_failure(&state, &request_id.0, "read report sequence", &failure);
        }
    };
    if sequence_conflict.is_some() {
        return refuse_sequence_taken(&state, tx, &auth.agent_id, &request_id.0).await;
    }
    if let Some(active) = &agent.active_boot_id {
        if active != &parsed.boot_id.to_string() {
            if old_boot_status.as_deref() == Some("closed")
                && parsed.boot_transition != platpulse_core::BootTransition::DrainedPrevious
            {
                return store_rejected(
                    &state,
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::StaleBoot,
                        "Report belongs to a closed boot; only exact replay is accepted",
                    ),
                    &request_id.0,
                )
                .await;
            }
            if parsed.boot_transition != platpulse_core::BootTransition::DrainedPrevious
                || parsed
                    .previous_boot_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(active.as_str())
            {
                if let Err(failure) = record_security_event(&mut tx, &auth.agent_id).await {
                    return storage_error(&request_id.0, "record security event", &failure);
                }
                return store_rejected(
                    &state,
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::ConflictingBoot,
                        "Report belongs to a competing boot",
                    ),
                    &request_id.0,
                )
                .await;
            }
            if agent.active_boot_status != "closed" {
                if let Err(failure) = record_security_event(&mut tx, &auth.agent_id).await {
                    return storage_error(&request_id.0, "record security event", &failure);
                }
                return store_rejected(
                    &state,
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::ConflictingBoot,
                        "Previous boot has not completed its closing receipt",
                    ),
                    &request_id.0,
                )
                .await;
            }
        } else if agent.active_boot_status == "closing"
            || agent.active_boot_status == "closed"
            || agent
                .last_report_sequence
                .is_some_and(|last| parsed.report_sequence <= last as u64)
        {
            return store_rejected(
                &state,
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::StaleReport,
                    "Report sequence is not newer than the accepted report",
                ),
                &request_id.0,
            )
            .await;
        }
    }
    // Network admission is a whole-report verdict, so it is decided for every
    // Node before the report writes anything: a sequence gap or a completed
    // Transfer written ahead of it would be committed together with the
    // rejection receipt.
    let mut node_admission = NodeAdmission::default();
    for node in parsed.inventory.nodes() {
        // A purged Node ID is a permanent admission boundary (design §15.3,
        // ADR 0004, issue #170): the Owner already erased this Node's
        // projection, monitoring history, and Validator Links, and no later
        // Inventory revision, retry, restart, or background write may
        // reconstruct them. Reject the entry per Node so a valid sibling in
        // the same legal report still reports, and do not let a stale or
        // unknown Network key on the removed ID block the rest of the report.
        let purged = match crate::node_purge::is_purged(&mut tx, &node.node_id.to_string()).await {
            Ok(value) => value,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        if purged {
            node_admission.refuse_purged(node.node_id);
            continue;
        }
        let known = match sqlx::query_scalar::<_, String>(
            "SELECT network_key FROM networks WHERE network_key = $1",
        )
        .bind(node.network_key.as_str())
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        if known.is_none() {
            return store_rejected(
                &state,
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::NetworkKeyUnknown,
                    "Network key is not registered",
                ),
                &request_id.0,
            )
            .await;
        }
    }
    // The report is admitted from here on; only per-Node refusals remain.
    if let Some(last) = agent.last_report_sequence
        && parsed.boot_id.to_string() == agent.active_boot_id.as_deref().unwrap_or_default()
        && parsed.report_sequence > (last as u64).saturating_add(1)
    {
        // Every statement in the ingestion transaction must succeed: a backend
        // that aborts the transaction on a failed statement (ADR 0013) would
        // otherwise fail the whole commit later with no clue why.
        if let Err(failure) = sqlx::query("INSERT INTO report_sequence_gaps (agent_id, boot_id, from_sequence, to_sequence, created_at) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING")
            .bind(&auth.agent_id)
            .bind(parsed.boot_id.to_string())
            .bind(last.saturating_add(1))
            .bind(parsed.report_sequence.saturating_sub(1) as i64)
            .bind(now().to_string())
            .execute(&mut *tx)
            .await
        {
            return storage_error(&request_id.0, "record report sequence gap", &failure);
        }
    }
    let now_text = now().to_string();
    // Every observation instant this Report contributes is read through the
    // stamp: the Report's own measured skew turns Agent-clock instants into
    // Server-clock ones, and an unmeasured skew falls back to the same-Report
    // relative time received + (observed − generated_at) (ADR 0014, #398).
    let stamp = ReportStamp::new(
        &now_text,
        parsed.host.clock_skew.latest,
        parsed.generated_at,
    );
    let capabilities =
        serde_json::to_string(&parsed.agent_capabilities).expect("capabilities serialize");
    let mut ownership_contradiction = false;
    for node in parsed.inventory.nodes() {
        if !node_admission.admits(node.node_id) {
            // Purged in the Network admission pass above.
            continue;
        }
        let owner =
            match sqlx::query_scalar::<_, String>("SELECT agent_id FROM nodes WHERE node_id = $1")
                .bind(node.node_id.to_string())
                .fetch_optional(&mut *tx)
                .await
            {
                Ok(v) => v,
                Err(_) => {
                    return error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            };
        if owner.is_some_and(|owner| owner != auth.agent_id) {
            // The reporting Agent declared a Node owned by another Agent.
            // Only a pending transfer targeting this Agent with a validated
            // Network Identity may switch ownership (design §4.6); every
            // other outcome keeps the source authoritative and is a
            // contradiction worth a security event.
            match resolve_node_transfer(&mut tx, &parsed, node, &auth.agent_id, &now_text).await {
                Ok(TransferResolution::Completed) => {
                    // Ownership switched atomically; this Node is no longer a
                    // mismatch and its current/samples join this report.
                }
                Ok(TransferResolution::Unverified) => {
                    // Legitimate in-flight declaration without an identity
                    // probe yet: the transfer stays pending and the Node
                    // entry stays rejected for this report.
                    node_admission.refuse_ownership(node.node_id);
                }
                Ok(TransferResolution::NoTransfer)
                | Ok(TransferResolution::NetworkKeyMismatch)
                | Ok(TransferResolution::IdentityMismatch) => {
                    node_admission.refuse_ownership(node.node_id);
                    ownership_contradiction = true;
                }
                Err(_) => {
                    return error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            }
        }
    }
    // A non-owner declaration without a valid pending transfer (or with a
    // rejected/contradicting transfer) is a security conflict: the source
    // Agent's later submissions after a completed Transfer also land here
    // and must be visible and auditable (design §4.6). The counter is
    // recorded once per report together with block identity mismatches.
    // Inventory is accepted as a complete set only after structural/network
    // validation. Ownership-invalid Nodes reject only their own current and
    // samples; they are not allowed to retire valid siblings.
    // A Node is inadmissible when it belongs to another Agent or its ID was
    // explicitly purged. Both are per-Node outcomes; the ordinary ownership
    // rejection still records its security event below, while a purge is a
    // stable, non-security disposition.
    let accepted_inventory_ids = parsed
        .inventory
        .nodes()
        .iter()
        .filter(|node| node_admission.admits(node.node_id))
        .map(|node| node.node_id.to_string())
        .collect::<Vec<_>>();

    // Equal content is unchanged; do not retire siblings or rewrite inventory
    // ownership on a replay. A newly accepted declaration remains authoritative.
    if !inventory_unchanged && parsed.inventory.nodes().is_empty() {
        if sqlx::query("UPDATE nodes SET lifecycle='retired', updated_at=$1 WHERE agent_id=$2")
            .bind(&now_text)
            .bind(&auth.agent_id)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    } else if !inventory_unchanged && !accepted_inventory_ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", accepted_inventory_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE nodes SET lifecycle='retired', updated_at=? WHERE agent_id=? AND node_id NOT IN ({placeholders})"
        );
        let sql = crate::database::number_placeholders(&sql);
        let mut query = sqlx::query(&sql).bind(&now_text).bind(&auth.agent_id);
        for node_id in &accepted_inventory_ids {
            query = query.bind(node_id);
        }
        if query.execute(&mut *tx).await.is_err() {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    }
    for node in parsed.inventory.nodes() {
        if !node_admission.admits(node.node_id) {
            continue;
        }
        let result = sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, $3, $4, $5, 'active', 'private', $6, $7, $8) ON CONFLICT(node_id) DO UPDATE SET network_key=excluded.network_key, display_name=COALESCE(nodes.display_name, excluded.display_name), rpc_endpoint=excluded.rpc_endpoint, lifecycle='active', inventory_revision=excluded.inventory_revision, updated_at=excluded.updated_at WHERE nodes.agent_id=excluded.agent_id")
            .bind(node.node_id.to_string()).bind(&auth.agent_id).bind(node.network_key.as_str()).bind(&node.display_name).bind(node.rpc_endpoint.as_str()).bind(effective_inventory_revision as i64).bind(&now_text).bind(&now_text).execute(&mut *tx).await;
        match result {
            Ok(done) if done.rows_affected() == 1 => {}
            // The projection only ever follows its owner: a row another Agent
            // owns is never overwritten here, even if a writer got past the
            // Node fence. The receipt is refused whole, so nothing of this
            // decision commits and the Agent's retry is judged against the
            // owner it can now see (issue #280).
            Ok(_) => {
                tracing::warn!(
                    node_id = %node.node_id,
                    "node owner changed between admission and projection; report refused for retry"
                );
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Node ownership changed while the report was admitted; retry the report",
                );
            }
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        }
        // Record the relations the Server accepted with this Report (issue
        // #221). A first Report opens the intervals; a change of reporting
        // Agent or Network key closes the old interval and opens the new one,
        // so history is never read from the Node's current columns.
        if crate::relationships::record_relations(
            &mut tx,
            &node.node_id.to_string(),
            &auth.agent_id,
            node.network_key.as_str(),
            &now_text,
        )
        .await
        .is_err()
        {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    }
    let mut projection_report = parsed.clone();
    projection_report
        .nodes
        .retain(|node| node_admission.admits(node.node_id));
    // Block history is admitted on its own below, sample by sample.
    projection_report.block_summaries.clear();
    projection_report.history_gaps.clear();
    let geo_provider = state.geo_config().provider;
    // The low-space gate is read once from memory for this Report. It is never
    // read through the pool: ingestion already holds the single write
    // connection, and a second acquire inside the transaction would deadlock.
    let history = state.capacity().history_gate();
    if let Err(save_error) =
        save_current(&mut tx, &history, &projection_report, stamp, geo_provider).await
    {
        return storage_error(&request_id.0, "save current observations", &save_error);
    }
    // Every Block Summary is judged once and written as it is admitted; the
    // Receipt below reads the same outcomes (ADR 0011 decision 4).
    let block_admission = match admit(&mut tx, &parsed, &node_admission, &now_text, |at| {
        stamp.history_at(at)
    })
    .await
    {
        Ok(admission) => admission,
        Err(failure) => return storage_error(&request_id.0, "admit block history", &failure),
    };
    if let Some((settlement, elapsed)) = block_admission.settlement {
        observe_gap_settlement(&state.metrics(), "report", settlement, elapsed);
    }
    if block_admission.identity_contradicted {
        // Identity mismatch is a typed, audited outcome (design §4.5): the
        // samples are never merged and the Agent's security counter records
        // the contradiction so the Admin surface can surface it.
        ownership_contradiction = true;
    }
    if ownership_contradiction {
        if let Err(failure) = record_security_event(&mut tx, &auth.agent_id).await {
            return storage_error(&request_id.0, "record security event", &failure);
        }
    }
    // Only admitted Nodes feed the Network-scoped reference head; a purged
    // Node's unvalidated Network key must not reach shared Network state.
    let network_keys = projection_report
        .inventory
        .nodes()
        .iter()
        .filter(|node| node_admission.admits(node.node_id))
        .map(|node| node.network_key.to_string())
        .collect::<Vec<_>>();
    if update_network_references(&mut tx, &network_keys, &now_text)
        .await
        .is_err()
    {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let nodes: Vec<NodeReceipt> = parsed
        .inventory
        .nodes()
        .iter()
        .map(|node| {
            let rejected = !node_admission.admits(node.node_id);
            NodeReceipt {
                node_id: node.node_id,
                current: if rejected {
                    NodeCurrentDisposition::Rejected
                } else {
                    NodeCurrentDisposition::Accepted
                },
                accepted_component_revisions: if rejected {
                    vec![]
                } else if let Some(observed) = parsed
                    .nodes
                    .iter()
                    .find(|candidate| candidate.node_id == node.node_id)
                {
                    let mut revisions = vec![
                        ComponentRevision {
                            component: ComponentKey::Process,
                            state_revision: observed.process.state_revision,
                            value_revision: observed.process.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Rpc,
                            state_revision: observed.chain.rpc.state_revision,
                            value_revision: observed.chain.rpc.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Sync,
                            state_revision: observed.chain.sync.state_revision,
                            value_revision: observed.chain.sync.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Consensus,
                            state_revision: observed.chain.consensus.state_revision,
                            value_revision: observed.chain.consensus.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::NetworkIdentity,
                            state_revision: observed.chain.network_identity.state_revision,
                            value_revision: observed.chain.network_identity.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::StaticMetadata,
                            state_revision: observed.chain.static_metadata.state_revision,
                            value_revision: observed.chain.static_metadata.value_revision,
                        },
                    ];
                    if let Some(data_directory_size) = &observed.data_directory_size_bytes {
                        revisions.push(ComponentRevision {
                            component: ComponentKey::DataDirectorySizeBytes,
                            state_revision: data_directory_size.state_revision,
                            value_revision: data_directory_size.value_revision,
                        });
                    }
                    if let Some(data_directory_capacity) = &observed.data_directory_capacity_bytes {
                        revisions.push(ComponentRevision {
                            component: ComponentKey::DataDirectoryCapacityBytes,
                            state_revision: data_directory_capacity.state_revision,
                            value_revision: data_directory_capacity.value_revision,
                        });
                    }
                    if let Some(peers) = &observed.chain.peers {
                        revisions.push(ComponentRevision {
                            component: ComponentKey::Peers,
                            state_revision: peers.state_revision,
                            value_revision: peers.value_revision,
                        });
                    }
                    revisions
                } else {
                    vec![]
                },
                rejections: node_admission
                    .refusal(node.node_id)
                    .map(Refusal::rejection)
                    .into_iter()
                    .collect(),
            }
        })
        .collect();
    let samples = parsed
        .block_summaries
        .iter()
        .zip(&block_admission.samples)
        .map(|(sample, outcome)| {
            disposition(
                sample.node_id,
                SampleRef::Block {
                    height: sample.block_number,
                },
                outcome.rejection(),
            )
        })
        .chain(
            parsed
                .history_gaps
                .iter()
                .zip(&block_admission.gaps)
                .map(|(gap, outcome)| {
                    disposition(
                        gap.node_id,
                        SampleRef::Gap {
                            from_height: gap.from_height,
                            to_height: gap.to_height,
                        },
                        outcome.rejection(),
                    )
                }),
        )
        .collect::<Vec<_>>();
    let inventory_changed = !inventory_unchanged;
    let inventory_disposition = if inventory_unchanged {
        InventoryDisposition::Unchanged
    } else {
        InventoryDisposition::Accepted
    };
    let disposition = if nodes
        .iter()
        .any(|node| node.current == NodeCurrentDisposition::Rejected)
        || samples
            .iter()
            .any(|sample| sample.disposition != SampleDispositionKind::Accepted)
    {
        ReceiptDisposition::PartiallyAccepted
    } else {
        ReceiptDisposition::Accepted
    };
    let acceptance = if declared_revision.is_none() {
        Some(InventoryAcceptance {
            revision: effective_inventory_revision,
            fingerprint: inventory_hash.clone(),
        })
    } else {
        None
    };
    let receipt = if declared_revision.is_none() {
        IngestReceipt::V2(Box::new(ReportReceiptV2 {
            report_id: parsed.report_id,
            disposition,
            report_body_sha256: hash.clone(),
            server_version: crate::VERSION.to_owned(),
            supported_protocol_majors: SUPPORTED_PROTOCOL_MAJORS.to_vec(),
            server_time: now(),
            rotation_hint: None,
            inventory: Some(InventoryReceiptV2 {
                disposition: inventory_disposition,
                acceptance,
            }),
            rejections: vec![],
            nodes,
            samples,
        }))
    } else {
        IngestReceipt::V1(Box::new(ReportReceipt {
            report_id: parsed.report_id,
            disposition,
            report_body_sha256: hash.clone(),
            server_version: crate::VERSION.to_owned(),
            supported_protocol_majors: SUPPORTED_PROTOCOL_MAJORS.to_vec(),
            server_time: now(),
            rotation_hint: None,
            inventory: Some(inventory_disposition),
            rejections: vec![],
            nodes,
            samples,
        }))
    };
    match store_receipt(&mut tx, &parsed, &hash, &receipt, &now_text, None).await {
        Ok(StoreOutcome::Stored) => {}
        Ok(StoreOutcome::Replay(replay)) => return replay.into_response(&request_id.0),
        Ok(StoreOutcome::SequenceTaken) => {
            return refuse_sequence_taken(&state, tx, &auth.agent_id, &request_id.0).await;
        }
        Err(failure) => {
            return storage_failure(&state, &request_id.0, "store report receipt", &failure);
        }
    }
    let lifecycle_status = match parsed.boot_transition {
        platpulse_core::BootTransition::Closing => "closed",
        _ => "active",
    };
    // `agent_boots_one_active` permits at most one active/closing Boot per
    // (agent_id, agent_epoch). Activating a Boot must be a single atomic write
    // rather than one that relies on `agents.active_boot_status` and the Boot
    // row already agreeing: a manual repair or a crash/fork window can leave a
    // stale active row behind, and the new Boot's first `drained_previous`
    // report would then hard-fail the upsert forever (issue #179). Close every
    // other active/closing row in this epoch first. For any accepted report the
    // only such row is the previous Boot, which this transaction is replacing.
    if let Err(stale_failure) = sqlx::query(
        "UPDATE agent_boots SET status='closed', closed_at=COALESCE(closed_at, $1), updated_at=$2 \
         WHERE agent_id=$3 AND agent_epoch=$4 AND boot_id<>$5 AND status IN ('active','closing')",
    )
    .bind(&now_text)
    .bind(&now_text)
    .bind(&auth.agent_id)
    .bind(parsed.agent_epoch as i64)
    .bind(parsed.boot_id.to_string())
    .execute(&mut *tx)
    .await
    {
        return storage_error(&request_id.0, "close stale agent boot", &stale_failure);
    }
    let boot_upsert = sqlx::query("INSERT INTO agent_boots (agent_id, agent_epoch, boot_id, status, previous_boot_id, last_sequence, close_report_id, closed_at, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $7='closed' THEN $8 ELSE NULL END, CASE WHEN $9='closed' THEN $10 ELSE NULL END, $11, $12) ON CONFLICT(agent_id, agent_epoch, boot_id) DO UPDATE SET status=excluded.status, previous_boot_id=COALESCE(excluded.previous_boot_id, agent_boots.previous_boot_id), last_sequence=CASE WHEN agent_boots.last_sequence >= excluded.last_sequence THEN agent_boots.last_sequence ELSE excluded.last_sequence END, close_report_id=COALESCE(excluded.close_report_id, agent_boots.close_report_id), closed_at=COALESCE(excluded.closed_at, agent_boots.closed_at), updated_at=excluded.updated_at")
        .bind(&auth.agent_id)
        .bind(parsed.agent_epoch as i64)
        .bind(parsed.boot_id.to_string())
        .bind(lifecycle_status)
        .bind(parsed.previous_boot_id.map(|v| v.to_string()))
        .bind(parsed.report_sequence as i64)
        .bind(lifecycle_status)
        .bind(parsed.report_id.to_string())
        .bind(lifecycle_status)
        .bind(&now_text)
        .bind(&now_text)
        .bind(&now_text)
        .execute(&mut *tx)
        .await;
    if let Err(boot_failure) = boot_upsert {
        return storage_error(&request_id.0, "upsert agent boot", &boot_failure);
    }
    let clock_skew_ms = parsed.host.clock_skew.latest;
    let clock_status = match clock_skew_ms {
        Some(value) if value.abs() > crate::http::agent::CLOCK_UNRELIABLE_THRESHOLD_MS => {
            "clock_unreliable"
        }
        Some(_) => "known",
        None => "unknown",
    };
    let should_activate_new_boot = parsed.boot_transition
        == platpulse_core::BootTransition::DrainedPrevious
        && parsed
            .previous_boot_id
            .as_ref()
            .map(ToString::to_string)
            .as_deref()
            == agent.active_boot_id.as_deref();
    let shutdown_diag = parsed.host.spool.latest.as_ref();
    let shutdown_state = shutdown_diag
        .and_then(|v| v.shutdown_state.as_deref())
        .unwrap_or(match parsed.boot_transition {
            platpulse_core::BootTransition::Closing => "final_stored",
            _ => "running",
        });
    let shutdown_last_error = shutdown_diag
        .and_then(|value| value.shutdown_last_error.as_deref())
        .map(crate::redaction::redact_sensitive);
    let updated = sqlx::query("UPDATE agents SET active_boot_id=$1, active_boot_status=$2, previous_boot_id=$3, close_report_id=CASE WHEN $4='closed' THEN $5 ELSE close_report_id END, last_report_sequence=$6, last_inventory_revision=$7, inventory_sha256=$8, inventory_protocol_major=$9, last_received_at=$10, clock_skew_ms=$11, clock_status=$12, agent_capabilities_json=$13, shutdown_state=$14, shutdown_started_at=$15, shutdown_deadline_at=$16, shutdown_finished_at=$17, shutdown_unresolved_from=$18, shutdown_unresolved_to=$19, shutdown_last_error=$20, shutdown_forced=$21, shutdown_report_id=CASE WHEN $22='closed' THEN $23 ELSE shutdown_report_id END, shutdown_report_sequence=$24, shutdown_updated_at=$25, updated_at=$26 WHERE agent_id=$27")
        .bind(parsed.boot_id.to_string()).bind(lifecycle_status)
        .bind(parsed.previous_boot_id.map(|v| v.to_string()))
        .bind(lifecycle_status).bind(parsed.report_id.to_string())
        .bind(parsed.report_sequence as i64).bind(effective_inventory_revision as i64)
        .bind(inventory_hash.as_str()).bind(I::protocol_version() as i64)
        .bind(&now_text).bind(clock_skew_ms).bind(clock_status)
        .bind(capabilities)
        .bind(shutdown_state)
        .bind(shutdown_diag.and_then(|v| v.shutdown_started_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_deadline_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_finished_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_unresolved_range.map(|range| range.0 as i64)))
        .bind(shutdown_diag.and_then(|v| v.shutdown_unresolved_range.map(|range| range.1 as i64)))
        .bind(shutdown_last_error.as_deref())
        .bind(shutdown_diag.and_then(|v| v.shutdown_forced).unwrap_or(false) as i64)
        .bind(lifecycle_status)
        .bind(parsed.report_id.to_string())
        .bind(parsed.report_sequence as i64)
        .bind(&now_text)
        .bind(&now_text)
        .bind(&auth.agent_id)
        .execute(&mut *tx)
        .await;
    if let Err(update_failure) = updated {
        return storage_error(&request_id.0, "update agent report state", &update_failure);
    }
    if should_activate_new_boot {
        if let Err(failure) = sqlx::query("UPDATE agents SET active_boot_status='active', previous_boot_id=$1, close_applied_at=$2 WHERE agent_id=$3")
            .bind(parsed.previous_boot_id.map(|v| v.to_string()))
            .bind(&now_text)
            .bind(&auth.agent_id)
            .execute(&mut *tx)
            .await
        {
            return storage_error(&request_id.0, "activate new boot", &failure);
        }
        if let Err(failure) = sqlx::query("UPDATE agent_boots SET status='closed', closed_at=$1, updated_at=$2 WHERE agent_id=$3 AND agent_epoch=$4 AND boot_id=$5")
            .bind(&now_text).bind(&now_text).bind(&auth.agent_id)
            .bind(parsed.agent_epoch as i64)
            .bind(parsed.previous_boot_id.map(|v| v.to_string()))
            .execute(&mut *tx)
            .await
        {
            return storage_error(&request_id.0, "close previous boot", &failure);
        }
    }
    // Alert evaluation runs in the same transaction as the accepted
    // projection (design §8.3: Alert input and invalidation belong to the
    // report transaction), so a transition is never observed without its
    // accepted facts. The sweep re-evaluates the remaining subjects. Only
    // Nodes owned by this Agent may be evaluated: a partially accepted or
    // hostile report must never advance another Agent's Node alert state.
    // Fail safe: without ownership proof no reported Node is evaluated here;
    // the sweep re-evaluates owned subjects on its own schedule.
    let owned: Vec<String> =
        match sqlx::query_scalar("SELECT node_id FROM nodes WHERE agent_id = $1")
            .bind(&auth.agent_id)
            .fetch_all(&mut *tx)
            .await
        {
            Ok(owned) => owned,
            Err(failure) => {
                return storage_error(&request_id.0, "load owned nodes", &failure);
            }
        };
    let node_ids: Vec<String> = parsed
        .nodes
        .iter()
        .map(|node| node.node_id.to_string())
        .filter(|node_id| owned.contains(node_id))
        .collect();
    let alert_changes = match crate::alerts::evaluate_report(
        &mut tx,
        &auth.agent_id,
        &node_ids,
        state.channels(),
        crate::auth::now_utc(),
    )
    .await
    {
        Ok(changes) => changes,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let peer_component_present = parsed.nodes.iter().any(|node| node.chain.peers.is_some());
    let public_node_ids = projection_report
        .nodes
        .iter()
        .map(|node| node.node_id.to_string())
        .collect::<Vec<_>>();
    let public_network_keys = projection_report
        .inventory
        .nodes()
        .iter()
        .map(|node| node.network_key.to_string())
        .collect::<HashSet<_>>();
    if let Err(commit_failure) = tx.commit().await {
        return storage_error(&request_id.0, "commit report transaction", &commit_failure);
    }
    // Retention sweeps (issues #213/#214/#217) are bounded hygiene rather than
    // part of the receipt, so they run on the crate::retention::run_worker
    // cadence instead of here. Sweeping inside this handler made a cleanup
    // statement and a concurrent Report lock the same node_metric_series_state
    // / host_metric_series_state rows in opposite orders, which PostgreSQL
    // resolved as a deadlock (40P01) whose victim could have been the Report.
    // Peer references are cache hygiene rather than part of the receipt too:
    // `crate::geo::cleanup_cache` refreshes them, drops the addresses no
    // current Peer references, and applies the cache bound on its own
    // maintenance cycle (issue #271). A Report neither scans nor rewrites the
    // Geo cache, so cache hygiene can never refuse one.
    // Every committed report can change the Owner-side Node/Agent health
    // projection (including host-only reports), while the Public projection
    // changes only when the report contains a Node observation.
    state
        .admin_realtime()
        .publish(AdminResource::Node, None::<String>);
    if inventory_changed || parsed.inventory.nodes().is_empty() {
        state
            .public_realtime()
            .publish(PublicResource::Node, None::<String>);
    }
    if !inventory_changed && !public_node_ids.is_empty() {
        for node_id in &public_node_ids {
            state
                .public_realtime()
                .publish(PublicResource::Node, Some(node_id.clone()));
        }
    }
    for network_key in &public_network_keys {
        state
            .public_realtime()
            .publish(PublicResource::Network, Some(network_key.clone()));
    }
    if peer_component_present {
        // A new Peer reference is a reason to resolve its country now; the
        // background path is woken instead of resolving inside this receipt.
        state.notify_geo_backfill();
        state
            .admin_realtime()
            .publish(AdminResource::Peer, None::<String>);
        state
            .admin_realtime()
            .publish(AdminResource::Geo, None::<String>);
        for network_key in &public_network_keys {
            state
                .public_realtime()
                .publish(PublicResource::Network, Some(network_key.clone()));
        }
    }
    if alert_changes > 0 {
        state
            .admin_realtime()
            .publish(AdminResource::Alerts, None::<String>);
    }
    receipt_response(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::report_ingestion::test_helpers::*;

    use std::sync::Arc;

    use axum::body::{Bytes, to_bytes};
    use axum::extract::{Extension, State};
    use axum::http::StatusCode;

    use crate::auth::AuthConfig;
    use crate::enrollment::{
        AgentAuthInfo, RECOVERY_TOKEN_DEFAULT_LIFETIME, create_recovery_token, recover,
    };
    use crate::http::{AppState, RequestId};

    use crate::secrets::load_pepper_file;

    use platpulse_core::{
        AgentReport, InventoryAcceptance, InventoryDeclaration, InventoryDisposition,
        ReceiptDisposition, ReportReceiptV2,
    };

    /// The two Nodes the v2 fixtures declare. A content change in these tests
    /// moves an RPC Endpoint, which the canonical declaration fingerprint covers.
    pub(super) const V2_NODE_A: &str = "0195f2a1-0014-4014-8014-000000000014";
    const V2_NODE_B: &str = "0195f2a1-0015-4015-8015-000000000015";
    const V2_BOOT_ID: &str = "0195f2a1-0012-4012-8012-000000000012";

    /// A v2 report built from the frozen v2 fixture. The declaration lists one
    /// Node per endpoint and the observation view is regenerated to match, so
    /// the report is a complete current view whose only declared change is the
    /// endpoint (or Node set).
    pub(super) fn v2_report_body(
        report_id: &str,
        sequence: u64,
        epoch: u64,
        boot_id: &str,
        endpoints: &[&str],
    ) -> Vec<u8> {
        let mut report: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v2_minimal.json"
        ))
        .unwrap();
        let template_node = report["inventory"]["nodes"][0].clone();
        let template_observation = report["nodes"][0].clone();
        let mut inventory_nodes = Vec::new();
        let mut observations = Vec::new();
        for (index, endpoint) in endpoints.iter().enumerate() {
            let node_id = if index == 0 { V2_NODE_A } else { V2_NODE_B };
            let mut node = template_node.clone();
            node["node_id"] = serde_json::json!(node_id);
            node["rpc_endpoint"] = serde_json::json!(endpoint);
            inventory_nodes.push(node);
            let mut observation = template_observation.clone();
            observation["node_id"] = serde_json::json!(node_id);
            observations.push(observation);
        }
        report["agent_epoch"] = serde_json::json!(epoch);
        report["report_sequence"] = serde_json::json!(sequence);
        report["report_id"] = serde_json::json!(report_id);
        report["boot_id"] = serde_json::json!(boot_id);
        report["inventory"]["nodes"] = serde_json::json!(inventory_nodes);
        report["nodes"] = serde_json::json!(observations);
        serde_json::to_vec(&report).unwrap()
    }

    pub(super) fn declaration_fingerprint(body: &[u8]) -> String {
        let report: AgentReport<InventoryDeclaration> = serde_json::from_slice(body).unwrap();
        report.inventory.fingerprint().to_string()
    }

    pub(super) async fn submit_v2_raw(
        state: &AppState,
        agent_id: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Vec<u8>) {
        let response = handler_v2(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.to_owned(),
                credential_id: "test-credential".to_owned(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(body),
        )
        .await;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, bytes.to_vec())
    }

    pub(super) async fn submit_v2(
        state: &AppState,
        agent_id: &str,
        body: Vec<u8>,
    ) -> ReportReceiptV2 {
        let (status, bytes) = submit_v2_raw(state, agent_id, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "unexpected v2 report response: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice::<ReportResponseV2>(&bytes)
            .unwrap()
            .receipt
    }

    pub(super) fn v2_acceptance(receipt: &ReportReceiptV2) -> InventoryAcceptance {
        receipt
            .inventory
            .as_ref()
            .expect("v2 receipt carries an Inventory outcome")
            .acceptance
            .clone()
            .expect("an appliable v2 receipt binds an acceptance pair")
    }

    pub(super) async fn accepted_inventory_revision(state: &AppState, agent_id: &str) -> i64 {
        sqlx::query_scalar("SELECT last_inventory_revision FROM agents WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap()
    }

    pub(super) async fn accepted_inventory_fingerprint(
        state: &AppState,
        agent_id: &str,
    ) -> Option<String> {
        sqlx::query_scalar("SELECT inventory_sha256 FROM agents WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap()
    }

    /// Issue #187: a failed write anywhere in the v2 receipt transaction must
    /// roll back the whole thing — allocation, projection, and receipt — and
    /// publish no post-commit invalidation. An injected trigger is the
    /// controlled failure source; the production path itself is untouched.
    pub(super) async fn v2_write_failure_rolls_back_all_writes(
        trigger_name: &str,
        timing: &str,
        operation: &str,
        table: &str,
        message: &str,
    ) {
        let (dir, state, agent_id) = state_with_agent().await;
        let body = v2_report_body(
            "0195f2a1-0240-4240-8240-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let events_before = state.admin_realtime().pending_events().len();
        crate::test_support::install_failure_trigger(
            state.db().pool(),
            trigger_name,
            timing,
            operation,
            table,
            message,
        )
        .await
        .unwrap();

        let (status, response) = submit_v2_raw(&state, &agent_id, body.clone()).await;
        assert_ne!(
            status,
            StatusCode::OK,
            "the injected failure must not be accepted: {}",
            String::from_utf8_lossy(&response)
        );

        assert_eq!(
            accepted_inventory_revision(&state, &agent_id).await,
            0,
            "a rollback must not leave an allocated revision"
        );
        assert_eq!(
            accepted_inventory_fingerprint(&state, &agent_id).await,
            None
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_report_receipts")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            0,
            "a rollback must not leave a receipt"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            0,
            "a rollback must not leave a projection"
        );
        assert_eq!(
            state.admin_realtime().pending_events().len(),
            events_before,
            "a rollback must publish no invalidation"
        );

        // Drop the trigger and reopen the real database: the rolled-back state
        // is durable, and no invalidation is replayed across the restart.
        crate::test_support::drop_failure_trigger(state.db().pool(), trigger_name, table)
            .await
            .unwrap();
        state.db().close().await;
        let database = crate::test_support::test_database_in(dir.path()).await;
        let auth = AuthConfig::development(
            load_pepper_file(&dir.path().join("pepper")).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let reopened = AppState::new(database, None, auth);
        assert_eq!(
            accepted_inventory_revision(&reopened, &agent_id).await,
            0,
            "a rollback must stay rolled back after reopening the database"
        );
        assert_eq!(
            accepted_inventory_fingerprint(&reopened, &agent_id).await,
            None
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_report_receipts")
                .fetch_one(reopened.db().pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes")
                .fetch_one(reopened.db().pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(reopened.admin_realtime().pending_events().len(), 0);

        // The database is still usable: the same report now commits revision 1.
        let recovered = submit_v2(&reopened, &agent_id, body).await;
        assert_eq!(v2_acceptance(&recovered).revision, 1);
        assert_eq!(accepted_inventory_revision(&reopened, &agent_id).await, 1);
    }

    /// A rejected report records no sequence gap: the next accepted report
    /// records the one gap that covers it, so Admin diagnostics count a single
    /// missing range instead of two overlapping ones.
    #[tokio::test]
    pub(super) async fn a_rejected_report_records_no_sequence_gap() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        let mut unknown_key = report.clone();
        unknown_key.report_sequence = 3;
        unknown_key.report_id = "0195f2a1-0029-4029-8029-000000000029".parse().unwrap();
        unknown_key.inventory.revision = 2;
        unknown_key.inventory.nodes[0].network_key = "platon-unregistered".parse().unwrap();
        unknown_key.validate().unwrap();
        let rejected = submit(&state, &agent_id, serde_json::to_vec(&unknown_key).unwrap()).await;
        assert_eq!(rejected.disposition, ReceiptDisposition::Rejected);
        let gaps = || async {
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT from_sequence, to_sequence FROM report_sequence_gaps WHERE agent_id=$1 ORDER BY gap_id",
            )
            .bind(&agent_id)
            .fetch_all(state.db().pool())
            .await
            .unwrap()
        };
        assert_eq!(gaps().await, Vec::<(i64, i64)>::new());

        report.report_sequence = 4;
        report.report_id = "0195f2a1-0030-4030-8030-000000000030".parse().unwrap();
        let accepted = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(accepted.disposition, ReceiptDisposition::Accepted);
        assert_eq!(gaps().await, vec![(2, 3)]);
    }

    /// A failed boot status read answers 503: it must not pass for an unseen
    /// boot and turn a database failure into a competing-boot security event.
    #[tokio::test]
    pub(super) async fn a_failed_boot_status_read_is_unavailable_not_a_competing_boot() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        sqlx::query("ALTER TABLE agent_boots RENAME TO agent_boots_unavailable")
            .execute(state.db().pool())
            .await
            .unwrap();

        report.boot_id = "0195f2a1-0031-4031-8031-000000000031".parse().unwrap();
        report.report_id = "0195f2a1-0032-4032-8032-000000000032".parse().unwrap();
        let status = submit_status(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let security_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(security_events, 0);
    }

    #[tokio::test]
    pub(super) async fn boot_sequence_gaps_and_competing_boots_are_recorded_and_rejected() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        report.report_sequence = 3;
        report.report_id = "0195f2a1-0026-4026-8026-000000000026".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let gaps: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM report_sequence_gaps WHERE agent_id=$1")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(gaps, 1);

        let old_boot = report.boot_id;
        report.boot_id = "0195f2a1-0027-4027-8027-000000000027".parse().unwrap();
        report.report_sequence = 1;
        report.report_id = "0195f2a1-0028-4028-8028-000000000028".parse().unwrap();
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(serde_json::to_vec(&report).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let receipt: ReportResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(receipt.receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.receipt.rejections[0].code,
            platpulse_core::RejectionCode::ConflictingBoot
        );
        assert_ne!(old_boot, report.boot_id);
    }

    #[tokio::test]
    pub(super) async fn conflicting_report_sequence_records_one_security_event() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;

        let mut conflict = original;
        conflict.report_id = "0195f2a1-0034-4034-8034-000000000034".parse().unwrap();
        conflict.host.cpu_percent.latest = Some(42.0);
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("sequence-conflict"))),
            Bytes::from(serde_json::to_vec(&conflict).unwrap()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT security_event_count FROM agents WHERE agent_id=$1",
            )
            .bind(&agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
    }

    /// A Report rejected for its own content still lands on its (Boot,
    /// sequence) slot. When another Report already holds that slot the
    /// rejection is a competing body, not a storage failure the Agent would
    /// retry forever.
    #[tokio::test]
    pub(super) async fn a_rejected_report_on_a_taken_sequence_is_a_conflicting_boot() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        report.report_sequence = 2;
        report.inventory.revision = 2;
        report.report_id = "0195f2a1-0041-4041-8041-000000000041".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        // Sequence 1 again under a new identity, declaring the revision the
        // Agent already moved past: rejected content on a taken slot.
        report.report_sequence = 1;
        report.inventory.revision = 1;
        report.report_id = "0195f2a1-0042-4042-8042-000000000042".parse().unwrap();
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(serde_json::to_vec(&report).unwrap()),
        )
        .await;
        let status = response.status();
        let error = body_json(response).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["error"]["code"], "conflicting_boot", "{error}");
        let security_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(security_events, 1);
        let stored: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM agent_report_receipts WHERE report_id=$1")
                .bind(report.report_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(stored, 0);
    }

    #[tokio::test]
    pub(super) async fn closing_then_drained_previous_atomically_rotates_boot_and_rejects_old_reports()
     {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut closing: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let old_boot = closing.boot_id;
        closing.report_sequence = 2;
        closing.report_id = "0195f2a1-0030-4030-8030-000000000030".parse().unwrap();
        closing.boot_transition = platpulse_core::BootTransition::Closing;
        submit(&state, &agent_id, serde_json::to_vec(&closing).unwrap()).await;

        let mut stale = closing.clone();
        stale.report_id = "0195f2a1-0031-4031-8031-000000000031".parse().unwrap();
        stale.report_sequence = 3;
        stale.boot_transition = platpulse_core::BootTransition::Continuing;
        let stale_receipt = submit(&state, &agent_id, serde_json::to_vec(&stale).unwrap()).await;
        assert_eq!(
            stale_receipt.rejections[0].code,
            platpulse_core::RejectionCode::StaleReport
        );

        let mut next = closing;
        next.boot_id = "0195f2a1-0032-4032-8032-000000000032".parse().unwrap();
        next.previous_boot_id = Some(old_boot);
        next.boot_transition = platpulse_core::BootTransition::DrainedPrevious;
        next.report_sequence = 1;
        next.report_id = "0195f2a1-0033-4033-8033-000000000033".parse().unwrap();
        let next_receipt = submit(&state, &agent_id, serde_json::to_vec(&next).unwrap()).await;
        assert_eq!(next_receipt.disposition, ReceiptDisposition::Accepted);

        let statuses = sqlx::query_as::<_, (String, String)>(
            "SELECT boot_id, status FROM agent_boots WHERE agent_id=$1 ORDER BY boot_id",
        )
        .bind(&agent_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &old_boot.to_string() && status == "closed")
        );
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &next.boot_id.to_string() && status == "active")
        );
        let security_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(security_events, 0);
    }

    // Issue #179: a manual repair (or a crash/fork window) can leave
    // `agents.active_boot_status = 'closed'` while the matching `agent_boots`
    // row is still `active`. The first report of the next Boot must close that
    // stale row inside the same transaction instead of hard-failing the
    // `agent_boots_one_active` upsert and answering a bare 503 forever.
    #[tokio::test]
    pub(super) async fn inconsistent_agents_boot_status_does_not_block_a_new_boot() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let old_boot = first.boot_id;
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;

        // Operator repair that only touched `agents`: the Boot row is stale.
        sqlx::query("UPDATE agents SET active_boot_status='closed' WHERE agent_id=$1")
            .bind(&agent_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let mut next = first.clone();
        next.boot_id = "0195f2a1-0094-4094-8094-000000000094".parse().unwrap();
        next.previous_boot_id = Some(old_boot);
        next.boot_transition = platpulse_core::BootTransition::DrainedPrevious;
        next.report_sequence = 1;
        next.report_id = "0195f2a1-0095-4095-8095-000000000095".parse().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&next).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);

        let statuses = sqlx::query_as::<_, (String, String)>(
            "SELECT boot_id, status FROM agent_boots WHERE agent_id=$1 ORDER BY boot_id",
        )
        .bind(&agent_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &old_boot.to_string() && status == "closed")
        );
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &next.boot_id.to_string() && status == "active")
        );
        let active: (Option<String>, String) = sqlx::query_as(
            "SELECT active_boot_id, active_boot_status FROM agents WHERE agent_id=$1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            active,
            (Some(next.boot_id.to_string()), "active".to_owned())
        );
    }

    // Issue #179: a database constraint failure during ingestion must not be
    // reported as a content-free `unavailable`; the operator needs a concrete
    // code to tell a constraint bug from a genuinely unavailable database.
    #[tokio::test]
    pub(super) async fn database_constraint_failures_are_not_reported_as_unavailable() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let failure = sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)")
            .bind(&agent_id)
            .bind("2026-08-12T08:00:00Z")
            .bind("2026-08-12T08:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap_err();
        let response = storage_error("test-request", "test constraint", &failure);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "storage_conflict");
        assert_eq!(body["error"]["requestId"], "test-request");
    }

    #[tokio::test]
    pub(super) async fn accepted_inventory_persists_observations_and_replay_is_exact() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let body =
            include_bytes!("../../../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        let first = submit(&state, &agent_id, body.to_vec()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        assert_eq!(first.inventory, Some(InventoryDisposition::Accepted));
        assert_eq!(first.nodes.len(), 1);
        let statuses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM component_status")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert!(statuses >= 13);
        let memory: i64 = sqlx::query_scalar(
            "SELECT memory_used_bytes FROM current_host_observations WHERE agent_id = $1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(memory, 4_294_967_296);
        let replay = submit(&state, &agent_id, body.to_vec()).await;
        assert_eq!(replay, first);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_report_receipts")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    pub(super) async fn equal_revision_inventory_conflict_does_not_mutate_node_projection() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;

        let mut conflict = original;
        conflict.report_sequence = 2;
        conflict.report_id = "0195f2a1-0013-4013-8013-000000000106".parse().unwrap();
        conflict.inventory.nodes[0].rpc_endpoint = "ws://127.0.0.1:6799".parse().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&conflict).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );

        let endpoint: String =
            sqlx::query_scalar("SELECT rpc_endpoint FROM nodes WHERE node_id=$1")
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(endpoint, "ws://127.0.0.1:6790");
    }

    #[tokio::test]
    pub(super) async fn equal_revision_inventory_conflict_records_durable_rejection_evidence() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let accepted = submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;
        assert_eq!(accepted.disposition, ReceiptDisposition::Accepted);

        let mut conflict = original.clone();
        conflict.report_sequence = 2;
        conflict.report_id = "0195f2a1-0013-4013-8013-000000000120".parse().unwrap();
        conflict.inventory.nodes[0].rpc_endpoint = "ws://127.0.0.1:6799".parse().unwrap();
        let reported_revision = conflict.inventory.revision;
        let reported_sha256 = conflict.inventory.content_sha256();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&conflict).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );

        // Issue #181: the refusal is durable Server evidence, not only a
        // decoded receipt body: the newest receipt names the deciding code
        // and the revision/hash the refused report declared.
        let (disposition, rejection_code, inventory_revision, inventory_sha256): (
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT disposition, rejection_code, inventory_revision, inventory_sha256 FROM agent_report_receipts WHERE agent_id=$1 ORDER BY received_at DESC, report_sequence DESC LIMIT 1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(disposition, "rejected");
        assert_eq!(
            rejection_code.as_deref(),
            Some("inventory_revision_conflict")
        );
        assert_eq!(inventory_revision, Some(reported_revision as i64));
        assert_eq!(
            inventory_sha256.as_deref(),
            Some(reported_sha256.as_str()),
            "the refused report's declared Inventory hash must be recorded"
        );

        // Issue #181: a rejection writes no projection row, so the Admin
        // invalidation is the only signal an already-open Agent page gets. An
        // accepted report publishes node/peer/geo work but never scopes an
        // `agent` event, so this event can only come from the refusal.
        assert!(
            state
                .admin_realtime()
                .pending_events()
                .iter()
                .any(|event| event.wire_resource() == "agent"
                    && event.resource_id.as_deref() == Some(agent_id.as_str())),
            "a rejected report must invalidate the Admin view of its Agent"
        );
    }

    #[tokio::test]
    pub(super) async fn stale_inventory_revision_is_rejected_without_lifecycle_mutation() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        original.inventory.revision = 2;
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;
        let mut stale = original.clone();
        stale.report_sequence = 2;
        stale.report_id = "0195f2a1-0013-4013-8013-000000000101".parse().unwrap();
        stale.inventory.revision = 1;
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("stale"))),
            Bytes::from(serde_json::to_vec(&stale).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let receipt: ReportResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(receipt.receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );
        let lifecycle: String =
            sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = $1")
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(lifecycle, "active");
    }

    pub(super) fn v2_with_transition(
        body: Vec<u8>,
        transition: platpulse_core::BootTransition,
        previous_boot_id: Option<&str>,
    ) -> Vec<u8> {
        let mut report: AgentReport<InventoryDeclaration> = serde_json::from_slice(&body).unwrap();
        report.boot_transition = transition;
        report.previous_boot_id = previous_boot_id.map(|id| id.parse().expect("valid boot id"));
        serde_json::to_vec(&report).unwrap()
    }

    /// Issue #187: a Server that committed a receipt but lost the response
    /// returns the *original* receipt for the identical report identity and
    /// bytes, even after a newer declaration was accepted. Different bytes
    /// under the same identity are a conflict.
    #[tokio::test]
    pub(super) async fn v2_replay_returns_the_original_receipt_and_identity_conflict_is_rejected() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let first_body = v2_report_body(
            "0195f2a1-0200-4200-8200-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let first = submit_v2(&state, &agent_id, first_body.clone()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        assert_eq!(v2_acceptance(&first).revision, 1);
        let first_fingerprint = v2_acceptance(&first).fingerprint;

        let second = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0200-4200-8200-000000000002",
                2,
                1,
                V2_BOOT_ID,
                &["ws://127.0.0.1:6791"],
            ),
        )
        .await;
        let second_fingerprint = v2_acceptance(&second).fingerprint;
        assert_ne!(first_fingerprint, second_fingerprint);
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);

        // The exact original bytes return the exact original receipt and move
        // nothing: no allocation, no projection, no revision change.
        let replayed = submit_v2(&state, &agent_id, first_body).await;
        assert_eq!(replayed, first, "the replay must be the original receipt");
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);
        assert_eq!(
            accepted_inventory_fingerprint(&state, &agent_id)
                .await
                .as_deref(),
            Some(second_fingerprint.as_str()),
            "a replay must not move the accepted declaration back"
        );

        let tampered = v2_report_body(
            "0195f2a1-0200-4200-8200-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6799"],
        );
        let (status, _body) = submit_v2_raw(&state, &agent_id, tampered).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "same identity with different bytes must conflict"
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);
    }

    /// Issue #187: the existing Epoch/Boot/Report Sequence fences stay
    /// authoritative. A late, older, or competing report cannot allocate a new
    /// revision merely because its content differs, while a legitimate newer
    /// report that restores earlier content is still a new declaration change.
    #[tokio::test]
    pub(super) async fn v2_ordering_fences_refuse_late_reports_without_allocating() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let content_a = "ws://127.0.0.1:6790";
        let content_b = "ws://127.0.0.1:6791";
        let content_c = "ws://127.0.0.1:6792";

        let first = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0210-4210-8210-000000000001",
                1,
                1,
                V2_BOOT_ID,
                &[content_a],
            ),
        )
        .await;
        assert_eq!(v2_acceptance(&first).revision, 1);
        let second = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0210-4210-8210-000000000002",
                3,
                1,
                V2_BOOT_ID,
                &[content_b],
            ),
        )
        .await;
        assert_eq!(v2_acceptance(&second).revision, 2);
        let accepted_before = accepted_inventory_fingerprint(&state, &agent_id).await;
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);

        // A late report with an unused but older sequence and new content is a
        // stale report: refused with no allocation.
        let stale = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0210-4210-8210-000000000003",
                2,
                1,
                V2_BOOT_ID,
                &[content_c],
            ),
        )
        .await;
        assert_eq!(stale.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            stale.rejections[0].code,
            platpulse_core::RejectionCode::StaleReport
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);
        assert_eq!(
            accepted_inventory_fingerprint(&state, &agent_id).await,
            accepted_before
        );

        // A stale Epoch is refused before any allocation.
        let old_epoch = v2_report_body(
            "0195f2a1-0210-4210-8210-000000000004",
            4,
            0,
            V2_BOOT_ID,
            &[content_c],
        );
        assert_eq!(
            submit_v2(&state, &agent_id, old_epoch).await.disposition,
            ReceiptDisposition::Rejected
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);

        // A competing Boot is refused before any allocation.
        let competing_boot = v2_report_body(
            "0195f2a1-0210-4210-8210-000000000005",
            4,
            1,
            "0195f2a1-0212-4212-8212-000000000099",
            &[content_c],
        );
        assert_eq!(
            submit_v2(&state, &agent_id, competing_boot)
                .await
                .disposition,
            ReceiptDisposition::Rejected
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);

        // A legitimate newer report that restores earlier content is a new
        // declaration change, not a reuse of the historical revision.
        let restored = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0210-4210-8210-000000000006",
                4,
                1,
                V2_BOOT_ID,
                &[content_a],
            ),
        )
        .await;
        assert_eq!(restored.disposition, ReceiptDisposition::Accepted);
        assert_eq!(v2_acceptance(&restored).revision, 3);
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 3);
    }

    /// Issue #187: the revision stays within the positive signed 64-bit range.
    /// At the ceiling an unchanged declaration is still accepted and a change
    /// that must increment fails explicitly, never wrapping or resetting.
    #[tokio::test]
    pub(super) async fn v2_revision_exhaustion_rejects_only_a_required_increment() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let body_a = v2_report_body(
            "0195f2a1-0220-4220-8220-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let fingerprint_a = declaration_fingerprint(&body_a);
        sqlx::query(
            "UPDATE agents SET last_inventory_revision = $1, inventory_sha256 = $2 WHERE agent_id = $3",
        )
        .bind(i64::MAX)
        .bind(&fingerprint_a)
        .bind(&agent_id)
        .execute(state.db().pool())
        .await
        .unwrap();

        let unchanged = submit_v2(&state, &agent_id, body_a).await;
        assert_eq!(unchanged.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            unchanged.inventory.as_ref().unwrap().disposition,
            InventoryDisposition::Unchanged
        );
        assert_eq!(v2_acceptance(&unchanged).revision, i64::MAX as u64);
        assert_eq!(
            accepted_inventory_revision(&state, &agent_id).await,
            i64::MAX
        );

        let changed = v2_report_body(
            "0195f2a1-0220-4220-8220-000000000002",
            2,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6791"],
        );
        let rejected = submit_v2(&state, &agent_id, changed).await;
        assert_eq!(rejected.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            rejected.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );
        assert_eq!(
            accepted_inventory_revision(&state, &agent_id).await,
            i64::MAX,
            "exhaustion must keep the existing revision"
        );
        assert_eq!(
            accepted_inventory_fingerprint(&state, &agent_id)
                .await
                .as_deref(),
            Some(fingerprint_a.as_str()),
            "exhaustion must keep the existing fingerprint"
        );
    }

    /// Issue #187: declaration comparison and version allocation live in one
    /// authoritative transaction. Two concurrent declarations that occupy the
    /// same sequence cannot both allocate: exactly one commits revision 1 and
    /// the other is fenced out by the existing sequence barrier.
    #[tokio::test]
    pub(super) async fn concurrent_v2_ingestion_never_shares_or_duplicates_a_revision() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let left = v2_report_body(
            "0195f2a1-0230-4230-8230-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let right = v2_report_body(
            "0195f2a1-0230-4230-8230-000000000002",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6791"],
        );
        let (left_result, right_result) = tokio::join!(
            submit_v2_raw(&state, &agent_id, left),
            submit_v2_raw(&state, &agent_id, right),
        );
        let statuses = [left_result.0, right_result.0];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::OK)
                .count(),
            1,
            "exactly one concurrent declaration may commit"
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CONFLICT)
                .count(),
            1,
            "the loser is refused by the sequence barrier"
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 1);
        let winner = if left_result.0 == StatusCode::OK {
            &left_result.1
        } else {
            &right_result.1
        };
        let receipt: ReportResponseV2 = serde_json::from_slice(winner).unwrap();
        assert_eq!(v2_acceptance(&receipt.receipt).revision, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM agent_report_receipts WHERE agent_id = $1"
            )
            .bind(&agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
    }

    /// Issue #187: a failed write anywhere in the v2 receipt transaction rolls
    /// back allocation, projection, and receipt, and emits no post-commit
    /// invalidation. An injected trigger is the failure source.
    #[tokio::test]
    pub(super) async fn v2_receipt_failure_rolls_back_allocation_and_projection() {
        v2_write_failure_rolls_back_all_writes(
            "abort_v2_receipt",
            "BEFORE",
            "INSERT",
            "agent_report_receipts",
            "injected receipt failure",
        )
        .await;
    }

    #[tokio::test]
    pub(super) async fn v2_projection_failure_rolls_back_allocation_and_receipt() {
        v2_write_failure_rolls_back_all_writes(
            "abort_v2_projection",
            "BEFORE",
            "INSERT",
            "nodes",
            "injected projection failure",
        )
        .await;
    }

    #[tokio::test]
    pub(super) async fn v2_allocation_failure_rolls_back_projection_and_receipt() {
        v2_write_failure_rolls_back_all_writes(
            "abort_v2_allocation",
            "BEFORE",
            "UPDATE",
            "agents",
            "injected allocation failure",
        )
        .await;
    }

    /// Issue #187: the accepted revision, fingerprint, and exact receipt are
    /// durable. A Server restart must return the original receipt and must not
    /// recompute the accepted pair against the current state.
    #[tokio::test]
    pub(super) async fn v2_accepted_revision_and_exact_replay_survive_a_server_restart() {
        let (dir, state, agent_id) = state_with_agent().await;
        let body = v2_report_body(
            "0195f2a1-0250-4250-8250-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let first = submit_v2(&state, &agent_id, body.clone()).await;
        let fingerprint = v2_acceptance(&first).fingerprint;
        state.db().close().await;

        let database = crate::test_support::test_database_in(dir.path()).await;
        let auth = AuthConfig::development(
            load_pepper_file(&dir.path().join("pepper")).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let restarted = AppState::new(database, None, auth);
        assert_eq!(accepted_inventory_revision(&restarted, &agent_id).await, 1);
        assert_eq!(
            accepted_inventory_fingerprint(&restarted, &agent_id)
                .await
                .as_deref(),
            Some(fingerprint.as_str())
        );
        assert_eq!(submit_v2(&restarted, &agent_id, body).await, first);
    }

    /// Issue #187: the revision belongs to the Agent identity. A Boot rotation
    /// (Closing then DrainedPrevious) and an Owner-authorized Agent Recovery
    /// (epoch advance) preserve it; only a genuine content change increments it.
    #[tokio::test]
    pub(super) async fn v2_revision_survives_boot_rotation_and_agent_recovery() {
        let (dir, state, agent_id) = state_with_agent().await;
        let content_a = "ws://127.0.0.1:6790";
        let content_b = "ws://127.0.0.1:6791";
        let first = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0260-4260-8260-000000000001",
                1,
                1,
                V2_BOOT_ID,
                &[content_a],
            ),
        )
        .await;
        assert_eq!(v2_acceptance(&first).revision, 1);

        let closing = v2_with_transition(
            v2_report_body(
                "0195f2a1-0260-4260-8260-000000000002",
                2,
                1,
                V2_BOOT_ID,
                &[content_a],
            ),
            platpulse_core::BootTransition::Closing,
            None,
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, closing).await).revision,
            1
        );

        let rotated_boot = "0195f2a1-0262-4262-8262-000000000062";
        let drained = v2_with_transition(
            v2_report_body(
                "0195f2a1-0260-4260-8260-000000000003",
                1,
                1,
                rotated_boot,
                &[content_b],
            ),
            platpulse_core::BootTransition::DrainedPrevious,
            Some(V2_BOOT_ID),
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, drained).await).revision,
            2
        );

        let unchanged_after_rotation = v2_report_body(
            "0195f2a1-0260-4260-8260-000000000004",
            2,
            1,
            rotated_boot,
            &[content_b],
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, unchanged_after_rotation).await).revision,
            2
        );

        // Owner-authorized Agent Recovery runs through the real enrollment path:
        // it advances the Agent Epoch and rotates the credential without
        // touching the accepted Inventory Revision.
        let pepper = load_pepper_file(&dir.path().join("pepper")).unwrap();
        let token = create_recovery_token(
            state.db(),
            &pepper,
            None,
            &agent_id,
            RECOVERY_TOKEN_DEFAULT_LIFETIME,
        )
        .await
        .unwrap();
        let recovered_agent = recover(state.db(), &pepper, &token.token).await.unwrap();
        assert_eq!(recovered_agent.agent_id, agent_id);
        assert_eq!(recovered_agent.agent_epoch, 2);
        let recovered = v2_report_body(
            "0195f2a1-0260-4260-8260-000000000005",
            3,
            2,
            rotated_boot,
            &[content_b],
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, recovered).await).revision,
            2
        );
        let changed = v2_report_body(
            "0195f2a1-0260-4260-8260-000000000006",
            4,
            2,
            rotated_boot,
            &[content_a],
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, changed).await).revision,
            3
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 3);
    }

    /// Issue #187: the revision belongs to one Agent identity. A second Agent
    /// identity has its own independent sequence, so one Agent's declaration
    /// change never advances another's revision.
    #[tokio::test]
    pub(super) async fn v2_distinct_agent_identities_keep_independent_revisions() {
        let (_dir, state, first_agent) = state_with_agent().await;
        let second_agent = "0195f2a1-0011-4011-8011-000000000022";
        sqlx::query(
            "INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)",
        )
        .bind(second_agent)
        .bind("2026-08-12T08:00:00Z")
        .bind("2026-08-12T08:00:00Z")
        .execute(state.db().pool())
        .await
        .unwrap();

        let first_body = v2_report_body(
            "0195f2a1-0270-4270-8270-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let mut second_value: serde_json::Value = serde_json::from_slice(&v2_report_body(
            "0195f2a1-0270-4270-8270-000000000002",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6791"],
        ))
        .unwrap();
        second_value["agent_id"] = serde_json::json!(second_agent);
        let second_body = serde_json::to_vec(&second_value).unwrap();

        let first = submit_v2(&state, &first_agent, first_body).await;
        assert_eq!(v2_acceptance(&first).revision, 1);
        let second = submit_v2(&state, second_agent, second_body).await;
        assert_eq!(
            v2_acceptance(&second).revision,
            1,
            "a new Agent identity starts its own sequence"
        );

        // A content change for the first identity advances only its revision.
        let first_changed = v2_report_body(
            "0195f2a1-0270-4270-8270-000000000003",
            2,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6799"],
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &first_agent, first_changed).await).revision,
            2
        );
    }
}
