//! The Closing Report: the one immutable Report that ends a Boot.
//!
//! Recovery, graceful shutdown and upgrade preparation each end a Boot with a
//! Closing, and all three build, validate, serialize and persist it here, so
//! the wire shape (protocol major, last-good values, pending samples, spool
//! diagnostics) cannot drift between them. They differ only in the Node
//! evidence the Closing carries:
//!
//! - [`store_probed_closing`] attempts every Node once more and keeps the
//!   last-good value of each component the attempt could not refresh
//!   (recovery: the Node may still be running);
//! - [`store_restated_closing`] never contacts a Node and restates each
//!   known Node exactly as the last durable Report described it (shutdown
//!   and preparation: the Agent is leaving and must finish when RPC is
//!   down, and a fresh failed attempt would report every graceful stop as
//!   an unreachable Node).
//!
//! Both keep the Host last-good values, so the Server can still correct the
//! Closing's observation instants with the last measured clock skew.

use platpulse_core::component::ComponentObservation;
use platpulse_core::identity::ReportId;
use platpulse_core::{AgentReport, BootTransition};

use crate::boot::BootState;
use crate::collector::{CollectionError, FailClosedRpcAdapter, RpcAdapter};
use crate::config::AgentConfig;
use crate::database::AgentStore;
use crate::shutdown_record::ShutdownRecord;

/// The identity of a Closing that is now durable in the spool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoredClosing {
    pub(crate) report_id: ReportId,
    pub(crate) report_sequence: u64,
}

/// Store the Closing of `boot` after attempting every Node once more.
pub(crate) async fn store_probed_closing<A: RpcAdapter>(
    store: &mut AgentStore,
    config: &AgentConfig,
    boot: &BootState,
    adapter: &A,
) -> Result<StoredClosing, CollectionError> {
    store_closing(
        store,
        config,
        boot,
        adapter,
        NodeEvidence::Probed,
        "Server time exchange was unavailable during recovery",
        None,
    )
    .await
}

/// Store the Closing of `boot` without contacting any Node. `note` records
/// the shutdown phase in the Closing's spool diagnostics.
pub(crate) async fn store_restated_closing(
    store: &mut AgentStore,
    config: &AgentConfig,
    boot: &BootState,
    note: ShutdownRecord,
) -> Result<StoredClosing, CollectionError> {
    store_closing(
        store,
        config,
        boot,
        &FailClosedRpcAdapter,
        NodeEvidence::Restated,
        "RPC collection is disabled during shutdown; persisted last-good values retained",
        Some(note),
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeEvidence {
    Probed,
    Restated,
}

async fn store_closing<A: RpcAdapter>(
    store: &mut AgentStore,
    config: &AgentConfig,
    boot: &BootState,
    adapter: &A,
    evidence: NodeEvidence,
    clock_skew_message: &str,
    note: Option<ShutdownRecord>,
) -> Result<StoredClosing, CollectionError> {
    let boot_id = boot.boot_id.ok_or(CollectionError::RecoveryRequired)?;
    let validated = config
        .validated_inventory()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let previous = crate::collector::load_last_report(store).await?;
    let at = crate::collector::timestamp();
    let data_directories = crate::collector::closing_directory_observations(
        &validated.data_directories,
        &validated.inventory,
        previous.as_ref(),
        at,
    );
    let mut report = crate::collector::collect_report_with_data_directories(
        config,
        boot.agent_id,
        boot.agent_epoch,
        boot_id,
        boot.report_sequence + 1,
        validated.inventory,
        adapter,
        crate::collector::clock_skew_error(at, clock_skew_message),
        &data_directories,
    )?;
    if let Some(previous) = previous.as_ref() {
        match evidence {
            NodeEvidence::Probed => report.keep_last_good(previous),
            NodeEvidence::Restated => restate(&mut report, previous),
        }
    }
    report.boot_transition = BootTransition::Closing;
    report.previous_boot_id = None;
    report.block_summaries = crate::block::load_block_summaries(store).await?;
    let monitored_nodes = report
        .inventory
        .nodes
        .iter()
        .map(|node| node.node_id)
        .collect::<Vec<_>>();
    report.history_gaps = crate::block::load_history_gaps(store, &monitored_nodes).await?;
    crate::collector::add_sample_capabilities(&mut report);
    report.host.spool = spool(store, note, report.generated_at).await?;
    report
        .validate()
        .map_err(crate::collector::report_validation_error)?;
    let body = crate::collector::report_body_bytes(&report, validated.server_managed_inventory)?;
    crate::reporting::persist_closing_report(store, &body, boot).await?;
    Ok(StoredClosing {
        report_id: report.report_id,
        report_sequence: report.report_sequence,
    })
}

/// Restate each Node the last durable Report described exactly as it was,
/// and keep the Host last-good values. A Node new to the Inventory keeps the
/// fail-closed attempt, which never fabricates a value.
fn restate(report: &mut AgentReport, previous: &AgentReport) {
    report.host.keep_last_good(&previous.host);
    for node in &mut report.nodes {
        if let Some(earlier) = previous
            .nodes
            .iter()
            .find(|earlier| earlier.node_id == node.node_id)
        {
            *node = earlier.clone();
        }
    }
}

async fn spool(
    store: &mut AgentStore,
    note: Option<ShutdownRecord>,
    at: platpulse_core::time::Rfc3339,
) -> Result<ComponentObservation<platpulse_core::observation::SpoolDiagnostics>, CollectionError> {
    let mut diagnostics = crate::collector::current_spool_diagnostics(store).await?;
    if let Some(note) = note {
        note.describe(&mut diagnostics);
    }
    Ok(crate::collector::ok(diagnostics, at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::AgentDatabaseConfig;
    use platpulse_core::component::ComponentStatus;
    use platpulse_core::inventory::InventoryDeclaration;

    const AGENT_ID: &str = "0195f2a1-0011-4011-8011-000000000011";
    const BOOT_ID: &str = "0195f2a1-0012-4012-8012-000000000012";

    /// The last durable Report: the shared minimal fixture, whose Host carries
    /// a measured clock skew and whose only Node is the configured one.
    fn last_good() -> AgentReport {
        serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap()
    }

    /// An enrolled Agent Store whose last durable Report is `last_good`, and
    /// a configuration that declares `inventory_revision` only when given.
    async fn enrolled(
        dir: &std::path::Path,
        inventory_revision: Option<u64>,
        last_good: Option<&AgentReport>,
    ) -> (AgentConfig, AgentStore, BootState) {
        let credential = dir.join("credential");
        crate::credential::write_credential_file(
            &credential,
            &format!("pp_agent_{AGENT_ID}_{}", "a".repeat(64)),
        )
        .unwrap();
        let db_path = dir.join("agent.db");
        let revision = inventory_revision
            .map(|revision| format!("inventory_revision={revision}\n"))
            .unwrap_or_default();
        let config_path = dir.join("agent.toml");
        std::fs::write(
            &config_path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\n{revision}nodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
                credential.display(),
                db_path.display()
            ),
        )
        .unwrap();
        let config = AgentConfig::resolve(&config_path).unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, last_report_body, updated_at) VALUES (1, ?, 1, ?, 1, ?, ?, ?)")
            .bind(AGENT_ID)
            .bind(BOOT_ID)
            .bind(inventory_revision.map_or(0, |revision| revision as i64))
            .bind(last_good.map(|report| serde_json::to_vec(report).unwrap()))
            .bind("2026-08-12T09:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        let boot = crate::boot::read(store.connection())
            .await
            .unwrap()
            .unwrap();
        (config, store, boot)
    }

    async fn stored_body(store: &mut AgentStore, closing: &StoredClosing) -> Vec<u8> {
        sqlx::query_scalar("SELECT body FROM reports WHERE report_id = ?")
            .bind(closing.report_id.to_string())
            .fetch_one(store.connection())
            .await
            .unwrap()
    }

    fn note() -> ShutdownRecord {
        let at = crate::collector::timestamp();
        ShutdownRecord::ClosingStored {
            started_at: at,
            deadline_at: at,
        }
    }

    /// Issue #397: a restated Closing keeps the Host last-good values (the
    /// clock skew the Server needs to place the copied Node observations in
    /// time) and restates the Node exactly as the last durable Report did.
    #[tokio::test]
    async fn a_restated_closing_keeps_the_host_last_good_and_restates_each_node() {
        let dir = tempfile::tempdir().unwrap();
        let last_good = last_good();
        let (config, mut store, boot) = enrolled(dir.path(), Some(1), Some(&last_good)).await;

        let closing = store_restated_closing(&mut store, &config, &boot, note())
            .await
            .unwrap();

        assert_eq!(closing.report_sequence, 2);
        let body = stored_body(&mut store, &closing).await;
        let report: AgentReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.boot_transition, BootTransition::Closing);
        assert_eq!(report.host.clock_skew.status, ComponentStatus::Error);
        assert_eq!(
            report.host.clock_skew.latest,
            last_good.host.clock_skew.latest
        );
        assert_eq!(report.nodes, last_good.nodes);
        let state = crate::boot::read(store.connection())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.report_sequence, 2);
        store.close().await.unwrap();
    }

    /// Issue #399: a Server-managed Agent's Closing uses protocol v2. The
    /// Server refuses a v1 Closing after the Inventory cutover, which would
    /// leave the Boot open until the next start recovers it.
    #[tokio::test]
    async fn a_server_managed_agent_stores_its_closing_under_protocol_v2() {
        let dir = tempfile::tempdir().unwrap();
        let last_good = last_good();
        let (config, mut store, boot) = enrolled(dir.path(), None, Some(&last_good)).await;

        let closing = store_restated_closing(&mut store, &config, &boot, note())
            .await
            .unwrap();

        let body = stored_body(&mut store, &closing).await;
        let report: AgentReport<InventoryDeclaration> = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.validate(), Ok(()));
        assert_eq!(report.boot_transition, BootTransition::Closing);
        assert_eq!(
            report.host.clock_skew.latest,
            last_good.host.clock_skew.latest
        );
        store.close().await.unwrap();
    }

    /// A Node never described by a durable Report has no last-good value to
    /// restate, so the Closing carries the fail-closed attempt instead of a
    /// fabricated one.
    #[tokio::test]
    async fn a_restated_closing_without_a_durable_report_fabricates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (config, mut store, boot) = enrolled(dir.path(), Some(1), None).await;

        let closing = store_restated_closing(&mut store, &config, &boot, note())
            .await
            .unwrap();

        let body = stored_body(&mut store, &closing).await;
        let report: AgentReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.host.clock_skew.latest, None);
        assert_eq!(report.nodes[0].chain.rpc.status, ComponentStatus::Error);
        assert_eq!(report.nodes[0].chain.rpc.latest, None);
        store.close().await.unwrap();
    }

    /// Issue #399: a recovery Closing carries the Block Summaries still pending
    /// in the Store, like every other Closing.
    #[tokio::test]
    async fn a_probed_closing_carries_the_pending_block_summaries() {
        use platpulse_core::AgentCapability;
        use platpulse_core::block::{BlockProductionAttribution, BlockSource, BlockSummary};

        let dir = tempfile::tempdir().unwrap();
        let last_good = last_good();
        let (config, mut store, boot) = enrolled(dir.path(), Some(1), Some(&last_good)).await;
        let hash = |byte: char| {
            format!("0x{}", byte.to_string().repeat(64))
                .parse()
                .unwrap()
        };
        let summary = BlockSummary {
            node_id: last_good.nodes[0].node_id,
            network_identity: last_good.nodes[0]
                .chain
                .network_identity
                .latest
                .clone()
                .unwrap(),
            block_number: 7,
            block_hash: hash('c'),
            parent_hash: hash('d'),
            block_timestamp_ms: 1_000,
            observed_at: last_good.generated_at,
            transaction_count: 2,
            block_interval_ms: None,
            source: BlockSource::Subscription,
            attribution: BlockProductionAttribution::unknown_attribution(
                format!("0x{}", "a".repeat(40)).parse().unwrap(),
                "test attribution",
            ),
        };
        crate::block::persist_block_summary(&mut store, &summary, "2026-08-12T09:00:00Z")
            .await
            .unwrap();

        let closing = store_probed_closing(&mut store, &config, &boot, &FailClosedRpcAdapter)
            .await
            .unwrap();

        let body = stored_body(&mut store, &closing).await;
        let report: AgentReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.block_summaries, vec![summary]);
        assert!(
            report
                .agent_capabilities
                .contains(&AgentCapability::BlockSummary)
        );
        store.close().await.unwrap();
    }

    /// A probed Closing attempts the Node again and keeps the last-good value
    /// of each component the attempt could not refresh, Host and Node alike.
    #[tokio::test]
    async fn a_probed_closing_keeps_last_good_values_through_a_failed_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let last_good = last_good();
        let (config, mut store, boot) = enrolled(dir.path(), Some(1), Some(&last_good)).await;

        let closing = store_probed_closing(&mut store, &config, &boot, &FailClosedRpcAdapter)
            .await
            .unwrap();

        let body = stored_body(&mut store, &closing).await;
        let report: AgentReport = serde_json::from_slice(&body).unwrap();
        let rpc = &report.nodes[0].chain.rpc;
        assert_eq!(rpc.status, ComponentStatus::Error);
        assert_eq!(rpc.latest, last_good.nodes[0].chain.rpc.latest);
        assert_eq!(
            report.host.clock_skew.latest,
            last_good.host.clock_skew.latest
        );
        store.close().await.unwrap();
    }
}
