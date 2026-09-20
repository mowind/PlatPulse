//! Owner-explicit permanent Node Purge (main design §15.3, ADR 0004, issue
//! #167).
//!
//! A Purge is not retirement, a visibility switch, or a recoverable soft
//! delete. In one serialized SQLite transaction it removes the Node's current
//! observation, monitoring history, and Node Validator Links and records the
//! minimal deletion identity. Shared Agent/Host/Network/Validator data,
//! existing Alert Incident evidence, and Audit are deliberately left in
//! place; independent Validator history is never deleted here even when this
//! was its last linked Node.
//!
//! The mutation is authoritative: it re-measures the impact inside the same
//! transaction that removes the rows, so a concurrent change cannot delete a
//! scope the Owner did not confirm, and a partial cleanup can never be
//! reported as a completed deletion because the row removal, the deletion
//! identity, and the Audit Event commit together.

use sqlx::{SqliteConnection, SqlitePool};

/// The Node identity an Owner is about to delete, read fresh inside the
/// purge transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePurgeTarget {
    pub node_id: String,
    pub agent_id: String,
    pub network_key: String,
    pub network_display_name: String,
    pub display_name: Option<String>,
    pub lifecycle: String,
    pub visibility: String,
    pub inventory_revision: i64,
    /// Server-native endpoint (callers redact it before exposing it).
    pub rpc_endpoint: String,
    pub first_seen_at: String,
    pub updated_at: String,
}

/// Everything a Node Purge removes, measured before the mutation so the
/// Owner confirms the same scope the Server is about to delete.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct NodePurgeCounts {
    pub component_statuses: i64,
    pub process_observations: i64,
    pub data_directory_observations: i64,
    pub chain_observations: i64,
    pub rpc_namespaces: i64,
    pub rpc_methods: i64,
    pub current_peers: i64,
    pub current_peer_capabilities: i64,
    pub peer_presence_intervals: i64,
    pub peer_aggregate_5m: i64,
    pub peer_aggregate_5m_countries: i64,
    pub peer_aggregate_1h: i64,
    pub peer_aggregate_1h_countries: i64,
    pub block_summaries: i64,
    pub block_history_states: i64,
    pub block_coverage_intervals: i64,
    pub block_identity_window: i64,
    pub block_history_gaps: i64,
    pub chain_divergence_observations: i64,
    pub observed_network_heads: i64,
    pub metric_samples: i64,
    pub validator_links: i64,
    pub validator_identity_status: i64,
    pub transfers: i64,
}

impl NodePurgeCounts {
    /// Accumulate another measured Node's counts into this one. Agent
    /// Removal aggregates the scope of every Node it purges.
    pub fn add(&mut self, other: &Self) {
        self.component_statuses += other.component_statuses;
        self.process_observations += other.process_observations;
        self.data_directory_observations += other.data_directory_observations;
        self.chain_observations += other.chain_observations;
        self.rpc_namespaces += other.rpc_namespaces;
        self.rpc_methods += other.rpc_methods;
        self.current_peers += other.current_peers;
        self.current_peer_capabilities += other.current_peer_capabilities;
        self.peer_presence_intervals += other.peer_presence_intervals;
        self.peer_aggregate_5m += other.peer_aggregate_5m;
        self.peer_aggregate_5m_countries += other.peer_aggregate_5m_countries;
        self.peer_aggregate_1h += other.peer_aggregate_1h;
        self.peer_aggregate_1h_countries += other.peer_aggregate_1h_countries;
        self.block_summaries += other.block_summaries;
        self.block_history_states += other.block_history_states;
        self.block_coverage_intervals += other.block_coverage_intervals;
        self.block_identity_window += other.block_identity_window;
        self.block_history_gaps += other.block_history_gaps;
        self.chain_divergence_observations += other.chain_divergence_observations;
        self.observed_network_heads += other.observed_network_heads;
        self.metric_samples += other.metric_samples;
        self.validator_links += other.validator_links;
        self.validator_identity_status += other.validator_identity_status;
        self.transfers += other.transfers;
    }

    /// Total Node-owned rows removed, excluding the nodes row itself.
    pub fn total_owned_rows(&self) -> i64 {
        self.component_statuses
            + self.process_observations
            + self.data_directory_observations
            + self.chain_observations
            + self.rpc_namespaces
            + self.rpc_methods
            + self.current_peers
            + self.current_peer_capabilities
            + self.peer_presence_intervals
            + self.peer_aggregate_5m
            + self.peer_aggregate_5m_countries
            + self.peer_aggregate_1h
            + self.peer_aggregate_1h_countries
            + self.block_summaries
            + self.block_history_states
            + self.block_coverage_intervals
            + self.block_identity_window
            + self.block_history_gaps
            + self.chain_divergence_observations
            + self.observed_network_heads
            + self.metric_samples
            + self.validator_links
            + self.validator_identity_status
            + self.transfers
    }
}

/// The target plus its Server-computed impact scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePurgeImpact {
    pub target: NodePurgeTarget,
    pub counts: NodePurgeCounts,
}

/// A committed Purge: the confirmed impact scope and the Server-owned
/// deletion instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePurgeOutcome {
    pub impact: NodePurgeImpact,
    pub deleted_at: String,
}

/// Read the Node identity and count everything a Purge would remove. Returns
/// Ok(None) when the Node does not exist (already purged, or never seen).
pub async fn measure(
    connection: &mut SqliteConnection,
    node_id: &str,
) -> Result<Option<NodePurgeImpact>, sqlx::Error> {
    let target = sqlx::query_as::<_, (String, String, String, Option<String>, String, String, i64, String, String, String)>(
        "SELECT n.node_id, n.agent_id, n.network_key, n.display_name, n.lifecycle, n.visibility, n.inventory_revision, n.rpc_endpoint, n.first_seen_at, n.updated_at FROM nodes n WHERE n.node_id = ?",
    )
    .bind(node_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((
        node_id,
        agent_id,
        network_key,
        display_name,
        lifecycle,
        visibility,
        inventory_revision,
        rpc_endpoint,
        first_seen_at,
        updated_at,
    )) = target
    else {
        return Ok(None);
    };
    let network_display_name =
        sqlx::query_scalar::<_, String>("SELECT display_name FROM networks WHERE network_key = ?")
            .bind(&network_key)
            .fetch_optional(&mut *connection)
            .await?
            .unwrap_or_else(|| network_key.clone());

    let counts = NodePurgeCounts {
        component_statuses: count(connection, "component_status", &node_id).await?,
        process_observations: count(connection, "current_node_process_observations", &node_id)
            .await?,
        data_directory_observations: count(
            connection,
            "current_node_data_directory_observations",
            &node_id,
        )
        .await?,
        chain_observations: count(connection, "current_node_chain_observations", &node_id).await?,
        rpc_namespaces: count(connection, "current_node_rpc_namespaces", &node_id).await?,
        rpc_methods: count(connection, "current_node_rpc_methods", &node_id).await?,
        current_peers: count(connection, "current_node_peers", &node_id).await?,
        current_peer_capabilities: count(connection, "current_node_peer_capabilities", &node_id)
            .await?,
        peer_presence_intervals: count(connection, "peer_presence_intervals", &node_id).await?,
        peer_aggregate_5m: count(connection, "peer_aggregate_5m", &node_id).await?,
        peer_aggregate_5m_countries: count(connection, "peer_aggregate_5m_countries", &node_id)
            .await?,
        peer_aggregate_1h: count(connection, "peer_aggregate_1h", &node_id).await?,
        peer_aggregate_1h_countries: count(connection, "peer_aggregate_1h_countries", &node_id)
            .await?,
        block_summaries: count(connection, "block_summaries", &node_id).await?,
        block_history_states: count(connection, "block_history_state", &node_id).await?,
        block_coverage_intervals: count(connection, "block_coverage_intervals", &node_id).await?,
        block_identity_window: count(connection, "block_identity_window", &node_id).await?,
        block_history_gaps: count(connection, "block_history_gaps", &node_id).await?,
        chain_divergence_observations: count(connection, "chain_divergence_observations", &node_id)
            .await?,
        observed_network_heads: count(connection, "observed_network_heads", &node_id).await?,
        metric_samples: count(connection, "node_metric_samples", &node_id).await?,
        validator_links: count(connection, "node_validator_links", &node_id).await?,
        validator_identity_status: count(connection, "node_validator_identity_status", &node_id)
            .await?,
        transfers: count(connection, "node_transfers", &node_id).await?,
    };

    Ok(Some(NodePurgeImpact {
        target: NodePurgeTarget {
            node_id,
            agent_id,
            network_key,
            network_display_name,
            display_name,
            lifecycle,
            visibility,
            inventory_revision,
            rpc_endpoint,
            first_seen_at,
            updated_at,
        },
        counts,
    }))
}

async fn count(
    connection: &mut SqliteConnection,
    table: &'static str,
    node_id: &str,
) -> Result<i64, sqlx::Error> {
    // The table name is a compile-time literal list, never user input.
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE node_id = ?");
    sqlx::query_scalar(&sql)
        .bind(node_id)
        .fetch_one(connection)
        .await
}

async fn delete(
    connection: &mut SqliteConnection,
    table: &'static str,
    node_id: &str,
) -> Result<(), sqlx::Error> {
    // The table name is a compile-time literal list, never user input.
    let sql = format!("DELETE FROM {table} WHERE node_id = ?");
    sqlx::query(&sql).bind(node_id).execute(connection).await?;
    Ok(())
}

/// Remove every Node-owned row and finally the nodes row itself.
///
/// Ordered children-first so the composite foreign keys (Peer capabilities
/// and the Peer aggregate country tables) can never block the parent delete.
/// Shared Agent/Host/Network/Validator tables, Alert Incidents, Notification
/// Events, Audit, and immutable Agent Report receipts are not referenced
/// here. The caller supplies the surrounding transaction.
///
/// Node Transfer rows are Node-owned management history and are removed with
/// the Node (their count is disclosed in the impact and Audit Event). A
/// pending Transfer is therefore gone with the Node, and the serialized report
/// ingestion transaction never re-admits the removed ID (issue #170).
/// Network-level projections such as network_reference_heads are keyed by
/// Network and are deliberately left untouched.
pub async fn remove(connection: &mut SqliteConnection, node_id: &str) -> Result<(), sqlx::Error> {
    for table in [
        // Composite children first.
        "current_node_peer_capabilities",
        "peer_aggregate_5m_countries",
        "peer_aggregate_1h_countries",
        // Current Node observation.
        "component_status",
        "current_node_process_observations",
        "current_node_data_directory_observations",
        "current_node_chain_observations",
        "current_node_rpc_namespaces",
        "current_node_rpc_methods",
        // Current and historical Peer state.
        "current_node_peers",
        "peer_presence_intervals",
        "peer_aggregate_5m",
        "peer_aggregate_1h",
        // Block and divergence monitoring history.
        "block_summaries",
        "block_history_state",
        "block_coverage_intervals",
        "block_identity_window",
        "block_history_gaps",
        "chain_divergence_observations",
        "observed_network_heads",
        // Metric samples.
        "node_metric_samples",
        // Node Validator Links, automatic-identity status, and management history.
        "node_validator_identity_status",
        "node_validator_links",
        "node_transfers",
    ] {
        delete(connection, table, node_id).await?;
    }
    sqlx::query("DELETE FROM nodes WHERE node_id = ?")
        .bind(node_id)
        .execute(connection)
        .await?;
    Ok(())
}

/// Persist the minimal deletion identity. This is the durable record that the
/// Node ID was explicitly purged; report ingestion reads it as the permanent
/// admission boundary (issue #170) that refuses every later declaration. The
/// caller supplies the Server-owned deletion instant.
pub async fn record_deletion_identity(
    connection: &mut SqliteConnection,
    target: &NodePurgeTarget,
    deleted_by_user_id: &str,
    deleted_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO deleted_nodes (node_id, agent_id, network_key, display_name, deleted_by_user_id, deleted_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&target.node_id)
    .bind(&target.agent_id)
    .bind(&target.network_key)
    .bind(target.display_name.as_deref())
    .bind(deleted_by_user_id)
    .bind(deleted_at)
    .execute(connection)
    .await?;
    Ok(())
}

/// Record the durable deletion identity and retire the Node as an alert and
/// notification subject in the same transaction (design §15.7, issue #175).
///
/// Every purge path goes through here, so a Node cannot be deleted without
/// leaving current evaluation, annotating its retained Incident evidence as
/// subject-deleted, and cancelling its not-yet-sent notifications. The caller
/// supplies the Server-owned deletion instant.
pub async fn record_deletion(
    connection: &mut SqliteConnection,
    target: &NodePurgeTarget,
    deleted_by_user_id: &str,
    deleted_at: &str,
) -> Result<crate::subject_deletion::SubjectRetirement, sqlx::Error> {
    record_deletion_identity(connection, target, deleted_by_user_id, deleted_at).await?;
    crate::subject_deletion::retire_subject(
        connection,
        crate::alerts::SubjectKind::Node,
        &target.node_id,
        deleted_at,
    )
    .await
}

/// Whether the Node ID carries the durable purge admission boundary.
///
/// Report ingestion reads this inside its receipt transaction and rejects the
/// Node per entry instead of reconstructing its projection (issue #170). The
/// table's shape and the boundary live here so ingestion never has to know it.
pub async fn is_purged(
    connection: &mut SqliteConnection,
    node_id: &str,
) -> Result<bool, sqlx::Error> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT node_id FROM deleted_nodes WHERE node_id = ?")
            .bind(node_id)
            .fetch_optional(connection)
            .await?
            .is_some(),
    )
}

/// Load the impact scope without any mutation. The preview and the mutation
/// share this measurement so they cannot disagree about what is Node-owned.
pub async fn preview(
    pool: &SqlitePool,
    node_id: &str,
) -> Result<Option<NodePurgeImpact>, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    measure(&mut connection, node_id).await
}

/// Execute an Owner-confirmed Purge inside one transaction: re-measure the
/// scope, remove every Node-owned row, persist the minimal deletion identity,
/// and append the Audit Event. Returns Ok(None) when the Node no longer
/// exists, so a replayed or raced mutation cannot delete an unseen scope.
pub async fn execute(
    pool: &SqlitePool,
    node_id: &str,
    actor_user_id: &str,
) -> Result<Option<NodePurgeOutcome>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let Some(impact) = measure(&mut transaction, node_id).await? else {
        transaction.rollback().await?;
        return Ok(None);
    };
    remove(&mut transaction, node_id).await?;
    let deleted_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    let retirement =
        record_deletion(&mut transaction, &impact.target, actor_user_id, &deleted_at).await?;

    let before = serde_json::json!({
        "agent_id": impact.target.agent_id,
        "network_key": impact.target.network_key,
        "display_name": impact.target.display_name,
        "lifecycle": impact.target.lifecycle,
        "inventory_revision": impact.target.inventory_revision,
    });
    let after = serde_json::json!({
        "deleted_at": deleted_at,
        "removed": impact.counts,
        "incidents_annotated": retirement.incidents_annotated,
        "evaluation_states_retired": retirement.evaluation_states_retired,
        "deliveries_cancelled": retirement.deliveries_cancelled,
    });
    crate::auth::insert_audit_change(
        &mut *transaction,
        Some(actor_user_id),
        "node_purged",
        "node",
        node_id,
        Some(&before),
        Some(&after),
    )
    .await?;
    transaction.commit().await?;
    Ok(Some(NodePurgeOutcome { impact, deleted_at }))
}
