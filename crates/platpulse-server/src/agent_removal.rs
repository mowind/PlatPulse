//! Owner-explicit Agent Removal (main design §15.2, ADR 0004, issue #171).
//!
//! Removing an Agent is deliberately distinct from Credential Revocation. In
//! one serialized SQLite transaction it revokes every credential, permanently
//! purges every Node the Agent authoritatively owns through the Node Purge
//! path, and persists a durable removal marker on the Agent registration
//! identity. The remote Agent/Node process is never contacted and no local
//! configuration is changed.
//!
//! The `agents` row is retained and marked (`deleted_at`), not physically
//! deleted: immutable Report receipts, Host/diagnostic projections, Transfer
//! history, and other evidence carry foreign keys to `agents`, and the
//! accepted contract keeps that evidence rather than cascading it away. The
//! marker is the authoritative removal boundary that list/detail reads and
//! every credential recovery path must honor, so Recovery, Rotation, an
//! outstanding Recovery Token, a late Report, or a restart cannot resurrect
//! the same identity.
//!
//! The mutation re-measures the owned-Node set inside the transaction and
//! compares it with the set the Owner confirmed, so a concurrent ownership
//! change cannot make the Server delete a Node the Owner never saw. A pending
//! Transfer (in either direction) blocks the removal until it is completed,
//! cancelled, or expires.

use std::collections::BTreeSet;

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::node_purge::{self, NodePurgeCounts};

/// The Agent registration identity an Owner is about to remove, read fresh
/// inside the mutation transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRemovalTarget {
    pub agent_id: String,
    pub display_name: Option<String>,
    pub notes: Option<String>,
    pub agent_epoch: i64,
    pub last_received_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, FromRow)]
struct AgentTargetRow {
    agent_id: String,
    display_name: Option<String>,
    notes: Option<String>,
    agent_epoch: i64,
    last_received_at: Option<String>,
    created_at: String,
    updated_at: String,
}

/// One Node the Agent authoritatively owns, resolved for the confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedNode {
    pub node_id: String,
    pub network_key: String,
    pub network_display_name: String,
    pub display_name: Option<String>,
    pub lifecycle: String,
    pub visibility: String,
    pub inventory_revision: i64,
}

#[derive(Debug, FromRow)]
struct OwnedNodeRow {
    node_id: String,
    network_key: String,
    network_display_name: String,
    display_name: Option<String>,
    lifecycle: String,
    visibility: String,
    inventory_revision: i64,
}

/// A Transfer that is still pending and involves this Agent as source or
/// target. Unhandled Transfers block a removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTransfer {
    pub transfer_id: String,
    pub node_id: String,
    pub source_agent_id: String,
    pub target_agent_id: String,
    /// `source` when this Agent owns the Node, `target` when the Transfer
    /// would hand the Node to this Agent.
    pub direction: String,
    pub expires_at: String,
}

#[derive(Debug, FromRow)]
struct PendingTransferRow {
    transfer_id: String,
    node_id: String,
    source_agent_id: String,
    target_agent_id: String,
    expires_at: String,
}

/// Everything an Agent Removal would do, measured before the mutation so the
/// Owner confirms the same scope the Server is about to erase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRemovalImpact {
    pub target: AgentRemovalTarget,
    pub owned_nodes: Vec<OwnedNode>,
    pub pending_transfers: Vec<PendingTransfer>,
    /// Aggregate Node-owned rows removed across every owned Node.
    pub counts: NodePurgeCounts,
    pub credential_count: i64,
    pub active_credential_count: i64,
}

/// One Node permanently purged as part of the removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgedNode {
    pub node_id: String,
    pub counts: NodePurgeCounts,
}

/// A committed Agent Removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRemovalResult {
    pub target: AgentRemovalTarget,
    pub purged_nodes: Vec<PurgedNode>,
    pub counts: NodePurgeCounts,
    pub revoked_credential_count: i64,
    pub deleted_at: String,
}

/// Why a confirmed removal did not remove the Agent. A refusal always leaves
/// the Server unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRemovalOutcome {
    /// Boxed because the committed result is much larger than a refusal.
    Removed(Box<AgentRemovalResult>),
    /// The Agent does not exist, was already removed, or is concurrently gone.
    NotFound,
    /// A pending Transfer must be completed, cancelled, or expired first.
    PendingTransfer(Vec<PendingTransfer>),
    /// The authoritative owned-Node set changed since the confirmation; the
    /// Owner must refetch and confirm again.
    OwnershipChanged { current_node_ids: Vec<String> },
}

/// Read the Agent identity and the complete removal impact. Returns Ok(None)
/// when the Agent does not exist or already carries the removal marker.
pub async fn measure(
    connection: &mut SqliteConnection,
    agent_id: &str,
) -> Result<Option<AgentRemovalImpact>, sqlx::Error> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let target = sqlx::query_as::<_, AgentTargetRow>(
        "SELECT agent_id, display_name, notes, agent_epoch, last_received_at, created_at, updated_at
         FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = target else {
        return Ok(None);
    };

    let owned_nodes = sqlx::query_as::<_, OwnedNodeRow>(
        "SELECT n.node_id, n.network_key,
                COALESCE(net.display_name, n.network_key) AS network_display_name,
                n.display_name, n.lifecycle, n.visibility, n.inventory_revision
         FROM nodes n
         LEFT JOIN networks net ON net.network_key = n.network_key
         WHERE n.agent_id = ?
         ORDER BY n.node_id",
    )
    .bind(agent_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|row| OwnedNode {
        node_id: row.node_id,
        network_key: row.network_key,
        network_display_name: row.network_display_name,
        display_name: row.display_name,
        lifecycle: row.lifecycle,
        visibility: row.visibility,
        inventory_revision: row.inventory_revision,
    })
    .collect::<Vec<_>>();

    let pending_transfers = sqlx::query_as::<_, PendingTransferRow>(
        "SELECT transfer_id, node_id, source_agent_id, target_agent_id, expires_at
         FROM node_transfers
         WHERE status = 'pending' AND expires_at > ?
           AND (source_agent_id = ? OR target_agent_id = ?)
         ORDER BY created_at, transfer_id",
    )
    .bind(&now)
    .bind(agent_id)
    .bind(agent_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|row| PendingTransfer {
        direction: if row.source_agent_id == agent_id {
            "source".to_owned()
        } else {
            "target".to_owned()
        },
        transfer_id: row.transfer_id,
        node_id: row.node_id,
        source_agent_id: row.source_agent_id,
        target_agent_id: row.target_agent_id,
        expires_at: row.expires_at,
    })
    .collect::<Vec<_>>();

    let mut counts = NodePurgeCounts::default();
    for node in &owned_nodes {
        // The same measurement a standalone Node Purge preview uses, so the
        // Agent confirmation cannot understate or overstate the Node scope.
        if let Some(impact) = node_purge::measure(&mut *connection, &node.node_id).await? {
            counts.add(&impact.counts);
        }
    }

    let credential_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_credentials WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_one(&mut *connection)
            .await?;
    let active_credential_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_credentials
         WHERE agent_id = ? AND revoked_at IS NULL
           AND (revoke_after IS NULL OR revoke_after > ?)",
    )
    .bind(agent_id)
    .bind(&now)
    .fetch_one(&mut *connection)
    .await?;

    Ok(Some(AgentRemovalImpact {
        target: AgentRemovalTarget {
            agent_id: row.agent_id,
            display_name: row.display_name,
            notes: row.notes,
            agent_epoch: row.agent_epoch,
            last_received_at: row.last_received_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        },
        owned_nodes,
        pending_transfers,
        counts,
        credential_count,
        active_credential_count,
    }))
}

/// Load the removal impact without any mutation.
pub async fn preview(
    pool: &SqlitePool,
    agent_id: &str,
) -> Result<Option<AgentRemovalImpact>, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    measure(&mut connection, agent_id).await
}

/// Execute an Owner-confirmed Agent Removal inside one transaction.
///
/// Refuses without side effects when the Agent is gone, when an unhandled
/// Transfer exists, or when the owned-Node set differs from
/// `confirmed_node_ids`. Otherwise it revokes every credential and
/// outstanding Recovery Token, purges every owned Node through the Node Purge
/// path, marks the Agent removed, and appends one Audit Event before the
/// commit makes the removal durable.
pub async fn execute(
    pool: &SqlitePool,
    agent_id: &str,
    confirmed_node_ids: &[String],
    actor_user_id: &str,
) -> Result<AgentRemovalOutcome, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let Some(impact) = measure(&mut transaction, agent_id).await? else {
        transaction.rollback().await?;
        return Ok(AgentRemovalOutcome::NotFound);
    };

    if !impact.pending_transfers.is_empty() {
        transaction.rollback().await?;
        return Ok(AgentRemovalOutcome::PendingTransfer(
            impact.pending_transfers,
        ));
    }

    let current_node_ids: Vec<String> = impact
        .owned_nodes
        .iter()
        .map(|node| node.node_id.clone())
        .collect();
    let confirmed: BTreeSet<&str> = confirmed_node_ids.iter().map(String::as_str).collect();
    let current: BTreeSet<&str> = current_node_ids.iter().map(String::as_str).collect();
    if confirmed != current {
        transaction.rollback().await?;
        return Ok(AgentRemovalOutcome::OwnershipChanged { current_node_ids });
    }

    let deleted_at = crate::auth::format_rfc3339(crate::auth::now_utc());

    // Revoke every credential (active or overlap-windowed) immediately. The
    // Agent row is marked below in the same transaction, so even a credential
    // that somehow survived would be refused by the removed-Agent boundary.
    let revoked = sqlx::query(
        "UPDATE agent_credentials SET revoked_at = ? WHERE agent_id = ? AND revoked_at IS NULL",
    )
    .bind(&deleted_at)
    .bind(agent_id)
    .execute(&mut *transaction)
    .await?;
    // An outstanding Recovery Token could otherwise mint a fresh credential
    // on the removed identity; revoke it here as well.
    sqlx::query(
        "UPDATE recovery_tokens SET revoked_at = ? WHERE agent_id = ? AND revoked_at IS NULL",
    )
    .bind(&deleted_at)
    .bind(agent_id)
    .execute(&mut *transaction)
    .await?;

    let mut purged_nodes = Vec::with_capacity(impact.owned_nodes.len());
    let mut counts = NodePurgeCounts::default();
    let mut retirement = crate::subject_deletion::SubjectRetirement::default();
    for node in &impact.owned_nodes {
        let Some(node_impact) = node_purge::measure(&mut transaction, &node.node_id).await? else {
            continue;
        };
        node_purge::remove(&mut transaction, &node.node_id).await?;
        let node_retirement = node_purge::record_deletion(
            &mut transaction,
            &node_impact.target,
            actor_user_id,
            &deleted_at,
        )
        .await?;
        retirement.add(&node_retirement);
        counts.add(&node_impact.counts);
        purged_nodes.push(PurgedNode {
            node_id: node.node_id.clone(),
            counts: node_impact.counts,
        });
    }

    let marked = sqlx::query(
        "UPDATE agents SET deleted_at = ?, deleted_by_user_id = ?, updated_at = ?
         WHERE agent_id = ? AND deleted_at IS NULL",
    )
    .bind(&deleted_at)
    .bind(actor_user_id)
    .bind(&deleted_at)
    .bind(agent_id)
    .execute(&mut *transaction)
    .await?;
    if marked.rows_affected() == 0 {
        // A concurrent removal won the race; delete nothing and report the
        // non-leaking not-found outcome.
        transaction.rollback().await?;
        return Ok(AgentRemovalOutcome::NotFound);
    }

    // The Agent and its Host subject leave current evaluation and notification
    // policy in the same commit as the removal marker; every purged Node was
    // already retired by the Node Purge path above (design §15.7, issue #175).
    for subject_kind in [
        crate::alerts::SubjectKind::Agent,
        crate::alerts::SubjectKind::Host,
    ] {
        let subject_retirement = crate::subject_deletion::retire_subject(
            &mut transaction,
            subject_kind,
            agent_id,
            &deleted_at,
        )
        .await?;
        retirement.add(&subject_retirement);
    }

    let before = serde_json::json!({
        "display_name": impact.target.display_name,
        "agent_epoch": impact.target.agent_epoch,
        "owned_node_ids": current_node_ids,
    });
    let after = serde_json::json!({
        "deleted_at": deleted_at,
        "revoked_credential_count": revoked.rows_affected(),
        "purged_node_count": purged_nodes.len(),
        "removed": counts,
        "incidents_annotated": retirement.incidents_annotated,
        "evaluation_states_retired": retirement.evaluation_states_retired,
        "deliveries_cancelled": retirement.deliveries_cancelled,
    });
    crate::auth::insert_audit_change(
        &mut *transaction,
        Some(actor_user_id),
        "agent_removed",
        "agent",
        agent_id,
        Some(&before),
        Some(&after),
    )
    .await?;

    transaction.commit().await?;
    Ok(AgentRemovalOutcome::Removed(Box::new(AgentRemovalResult {
        target: impact.target,
        purged_nodes,
        counts,
        revoked_credential_count: revoked.rows_affected() as i64,
        deleted_at,
    })))
}
