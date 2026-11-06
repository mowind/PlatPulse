//! Observed Network Head references.

use sqlx::{Any, Transaction};

/// Recompute the Network Reference Head of every Network this Report touched.
///
/// A source must have been *observed* within the window on the Server clock, not
/// merely delivered: a Node whose Reports are being replayed from a backlog is
/// Stale, and a Stale Node is never a source of the Observed Network Head
/// (ADR 0014). `now` is the receipt instant of the triggering
/// Report, which is the only instant the Server can defend when it finds no
/// eligible source at all.
pub(super) async fn update_network_references(
    tx: &mut Transaction<'_, Any>,
    network_keys: &[String],
    now: &str,
) -> Result<(), sqlx::Error> {
    // The head is recomputed from every Node of the Network, so it is a
    // read-then-write over rows other Agents' Reports write concurrently on
    // the PostgreSQL work connections (issue #280). Locking the Network's head
    // row before the read serializes the recomputations: a receipt waiting
    // here reads the Nodes the previous one committed (each statement of a
    // READ COMMITTED transaction takes a fresh snapshot), so the last head
    // written counts every source. The rows are locked in ascending key order
    // so two Reports touching the same Networks never wait on each other in
    // a cycle. On SQLite the single writer serializes this already, and the
    // no-op update costs nothing.
    let mut locked_keys = network_keys.iter().collect::<Vec<_>>();
    locked_keys.sort();
    locked_keys.dedup();
    for network_key in locked_keys {
        sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES ($1, NULL, $2, 'unknown', 0, NULL) ON CONFLICT(network_key) DO UPDATE SET confidence=network_reference_heads.confidence")
            .bind(network_key)
            .bind(now)
            .execute(&mut **tx)
            .await?;
    }
    let now_instant = crate::auth::parse_rfc3339(now);
    for network_key in network_keys {
        // A Node whose head was never observed is not a source: its NULL head
        // must not stand in as block 0 (the native SQLite decode once did).
        let heads = sqlx::query_as::<_, (String, i64, Option<String>)>(
            "SELECT n.node_id, c.current_block, h.resync_state FROM nodes n JOIN current_node_chain_observations c ON c.node_id=n.node_id LEFT JOIN block_history_state h ON h.node_id=n.node_id WHERE n.network_key=$1 AND n.lifecycle='active' AND c.current_block IS NOT NULL ORDER BY c.current_block DESC",
        )
        .bind(network_key)
        .fetch_all(&mut **tx)
        .await?;
        let node_ids = heads
            .iter()
            .map(|(node_id, _, _)| node_id.as_str())
            .collect::<Vec<_>>();
        let evidence = crate::node_currency::load(tx, &node_ids).await?;
        // A source must be a Node whose Node Currency is Current, the same
        // judgement the Public projection makes; an unparseable receipt
        // instant leaves no source at all.
        let rows = heads
            .iter()
            .filter_map(|(node_id, head, resync_state)| {
                let now = now_instant?;
                let evidence = evidence.get(node_id)?;
                if evidence.currency(now) != crate::node_currency::NodeCurrency::Current {
                    return None;
                }
                let sync_at = evidence
                    .observed_at(crate::node_currency::GatingComponent::Sync)
                    .map(crate::auth::format_rfc3339);
                Some((node_id, *head, resync_state, sync_at))
            })
            .collect::<Vec<_>>();
        let eligible = rows
            .iter()
            .filter(|(_, _, state, _)| state.as_deref() != Some("resyncing"))
            .collect::<Vec<_>>();
        let candidates = if eligible.is_empty() {
            rows.iter().collect::<Vec<_>>()
        } else {
            eligible
        };
        // The head is a sync fact, so the instant it was observed is the sync
        // component's observation instant.
        let Some((node_id, head, _, sync_at)) = candidates.first() else {
            // No Node was observed recently enough to hold a reference: the row
            // records that the Server looked at its own clock `now` and found
            // none, which is why the reference is Unknown rather than stale.
            sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES ($1, NULL, $2, 'unknown', 0, NULL) ON CONFLICT(network_key) DO UPDATE SET block_number=NULL, observed_at=excluded.observed_at, confidence='unknown', eligible_source_count=0, contributing_node_id=NULL")
                .bind(network_key).bind(now).execute(&mut **tx).await?;
            continue;
        };
        let confidence = if candidates.len() >= 2 { "high" } else { "low" };
        // The reference head is the head of the winning source at the instant it
        // was observed, not the instant this Report arrived.
        sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT(network_key) DO UPDATE SET block_number=excluded.block_number, observed_at=excluded.observed_at, confidence=excluded.confidence, eligible_source_count=excluded.eligible_source_count, contributing_node_id=excluded.contributing_node_id")
            .bind(network_key).bind(*head).bind(sync_at.as_deref().unwrap_or(now)).bind(confidence).bind(candidates.len() as i64).bind(node_id.as_str())
            .execute(&mut **tx).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::http::report_ingestion::test_helpers::*;

    use crate::http::AppState;

    use platpulse_core::component::ComponentStatus;

    use platpulse_core::{AgentReport, ReceiptDisposition, SampleDispositionKind};

    /// The Registry tuple of the Network `state_with_agent` registers.
    pub(super) fn registered_identity() -> platpulse_core::network::NetworkIdentity {
        platpulse_core::network::NetworkIdentity {
            genesis_hash: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            chain_id: 210425,
            p2p_network_id: 210425,
            address_hrp: Some("lat".to_owned()),
        }
    }

    /// The identity a Node served before its build upgrade: the legacy chain
    /// and p2p ids, and a genesis hash that is not the registered one.
    pub(super) fn pre_upgrade_identity() -> platpulse_core::network::NetworkIdentity {
        platpulse_core::network::NetworkIdentity {
            genesis_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap(),
            chain_id: 100,
            p2p_network_id: 100,
            address_hrp: Some("lat".to_owned()),
        }
    }

    /// A v1 report whose Node observation is `observed`, carrying one sample
    /// per requested height stamped with `stamp`. The observation is the
    /// Node's own fresh identity; the stamp is what the Agent cached and
    /// replays into every sample until its Block Worker reconnects.
    pub(super) fn report_with_stamped_samples(
        report_id: &str,
        observed: &platpulse_core::network::NetworkIdentity,
        stamp: &platpulse_core::network::NetworkIdentity,
        heights: &[u64],
    ) -> AgentReport {
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.report_id = report_id.parse().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let observation = &mut report.nodes[0].chain.network_identity;
        observation.status = ComponentStatus::Ok;
        observation.latest = Some(observed.clone());
        observation.latest_observed_at = Some(report.generated_at);
        observation.error = None;
        for height in heights {
            report
                .block_summaries
                .push(platpulse_core::block::BlockSummary {
                    node_id,
                    network_identity: stamp.clone(),
                    block_number: *height,
                    block_hash: format!("0x{:064x}", height + 1).parse().unwrap(),
                    parent_hash: format!("0x{:064x}", height).parse().unwrap(),
                    block_timestamp_ms: 1_000,
                    observed_at: report.generated_at,
                    transaction_count: 3,
                    block_interval_ms: None,
                    source: platpulse_core::block::BlockSource::Subscription,
                    attribution:
                        platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                            "0x1111111111111111111111111111111111111111"
                                .parse()
                                .unwrap(),
                            "test",
                        ),
                });
        }
        report.validate().unwrap();
        report
    }

    pub(super) async fn recorded_gap_reasons(
        state: &AppState,
        node_id: platpulse_core::identity::NodeId,
    ) -> Vec<(i64, i64, String)> {
        sqlx::query_as(
            "SELECT from_height, to_height, reason FROM block_history_gaps WHERE node_id=$1 ORDER BY from_height, to_height",
        )
        .bind(node_id.to_string())
        .fetch_all(state.db().pool())
        .await
        .unwrap()
    }

    /// The History Gap rows a Node carries with their author, in height order.
    pub(super) async fn recorded_gap_rows(
        state: &AppState,
        node_id: platpulse_core::identity::NodeId,
    ) -> Vec<(i64, i64, String, String)> {
        sqlx::query_as(
            "SELECT from_height, to_height, reason, authored_by FROM block_history_gaps WHERE node_id=$1 ORDER BY from_height, to_height",
        )
        .bind(node_id.to_string())
        .fetch_all(state.db().pool())
        .await
        .unwrap()
    }

    /// #235 decision 4: after a Node build upgrade the Node reports the
    /// registered Network while the Agent keeps stamping samples with the
    /// identity it cached. Every rejection must name the differing fields with
    /// their stamped and registered values, point at the Agent restart as the
    /// remedy, and leave the Server's own diagnosis as the History Gap reason
    /// operators read — the Agent's later declaration of the same gap may not
    /// replace it.
    #[tokio::test]
    pub(super) async fn stale_stamp_rejection_names_the_differing_fields_and_the_agent_restart() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000301",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10, 11],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.samples.len(), 2);
        for sample in &receipt.samples {
            assert_eq!(sample.disposition, SampleDispositionKind::TerminalRejected);
            let rejection = sample.rejection.as_ref().unwrap();
            assert_eq!(
                rejection.code,
                platpulse_core::RejectionCode::NetworkIdentityMismatch
            );
            assert!(!rejection.retryable);
            // The reason names every field that actually differed, on both
            // sides, and never invents one that matched.
            assert!(
                rejection.reason.contains(
                    "genesis_hash stamped 0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa registered 0x0000000000000000000000000000000000000000000000000000000000000001"
                ),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("chain_id stamped 100 registered 210425"),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("p2p_network_id stamped 100 registered 210425"),
                "{}",
                rejection.reason
            );
            assert!(
                !rejection.reason.contains("address_hrp"),
                "{}",
                rejection.reason
            );
            // ...and points at the stale side plus the remedy for it.
            assert!(
                rejection
                    .reason
                    .contains("restarting the Agent on that Host is the only remedy"),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("never edited to match a stamped sample"),
                "{}",
                rejection.reason
            );
        }

        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);

        // The Server recorded its own diagnosis for exactly the rejected
        // heights, one row per sample.
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 2);
        assert_eq!(gaps[0].0, 10);
        assert_eq!(gaps[0].1, 10);
        assert_eq!(gaps[1].0, 11);
        assert_eq!(gaps[1].1, 11);
        let reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(gaps[0].2, reason);
        assert_eq!(gaps[1].2, reason);

        // The Agent reports the same gap back with its own wording, as the
        // in-process Agent does once it applies the Receipt. The unique gap
        // identity folds it into the Server's row, so the diagnosis survives.
        let mut declared = report.clone();
        declared.report_sequence += 1;
        declared.report_id = "0195f2a1-0013-4013-8013-000000000302".parse().unwrap();
        declared.block_summaries.clear();
        declared.history_gaps = [10u64, 11]
            .into_iter()
            .map(|height| platpulse_core::gap::HistoryGap {
                node_id,
                kind: platpulse_core::gap::GapKind::ServerRejected,
                from_height: height,
                to_height: height,
                reason: "locally retained history gap".to_owned(),
                recorded_at: report.generated_at,
            })
            .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(recorded_gap_reasons(&state, node_id).await, gaps);
    }

    /// One Report can carry more than one Block Identity Stamp for the same
    /// Node: samples spooled across a subscription boundary keep the stamp
    /// they were collected with, and the wire format validates each sample's
    /// stamp on its own. Every rejection must name the fields that actually
    /// differed for *its* sample (ADR 0011 decision 4), and the History Gap
    /// the Server leaves behind must carry that same text.
    #[tokio::test]
    pub(super) async fn each_stamped_sample_is_diagnosed_on_its_own_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000305",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let mut hrp_stamp = registered_identity();
        hrp_stamp.address_hrp = Some("lax".to_owned());
        let mut second = report.block_summaries[0].clone();
        second.block_number = 11;
        second.block_hash = format!("0x{:064x}", 12).parse().unwrap();
        second.parent_hash = format!("0x{:064x}", 11).parse().unwrap();
        second.network_identity = hrp_stamp;
        report.block_summaries.push(second);
        report.validate().unwrap();

        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.samples.len(), 2);
        let rejection_at = |height: u64| {
            receipt
                .samples
                .iter()
                .find(|sample| {
                    sample.sample == platpulse_core::receipt::SampleRef::Block { height }
                })
                .and_then(|sample| sample.rejection.clone())
                .unwrap()
        };

        let legacy = rejection_at(10);
        assert_eq!(
            legacy.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            legacy.reason.contains("genesis_hash stamped 0xaaaa")
                && legacy
                    .reason
                    .contains("chain_id stamped 100 registered 210425")
                && !legacy.reason.contains("address_hrp"),
            "{}",
            legacy.reason
        );

        let hrp = rejection_at(11);
        assert_eq!(
            hrp.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            hrp.reason
                .contains("address_hrp stamped lax registered lat")
                && !hrp.reason.contains("chain_id")
                && !hrp.reason.contains("genesis_hash"),
            "{}",
            hrp.reason
        );

        // Each refused height carries its own sample's diagnosis instead of the
        // first sample's fields repeated for the whole Node.
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 2);
        assert_eq!((gaps[0].0, gaps[0].1), (10, 10));
        assert_eq!(gaps[0].2, legacy.reason);
        assert_eq!((gaps[1].0, gaps[1].1), (11, 11));
        assert_eq!(gaps[1].2, hrp.reason);
    }

    /// A Network registered under the literal HRP "unset" is a legal Registry
    /// tuple, and a sample whose stamp matches it must be accepted even when a
    /// sibling sample of the same Report carries no HRP at all. Before the
    /// diagnosis was keyed by an explicit field list both stamps produced the
    /// same key, so the matching summary was refused (and a permanent History
    /// Gap invented for it) while the data plane stored it anyway.
    #[tokio::test]
    pub(super) async fn samples_are_diagnosed_by_their_own_stamp_not_by_a_sibling_ones() {
        let (_dir, state, agent_id) = state_with_agent().await;
        sqlx::query("UPDATE networks SET address_hrp = 'unset'")
            .execute(state.db().pool())
            .await
            .unwrap();

        let mut registered = registered_identity();
        registered.address_hrp = Some("unset".to_owned());
        let mut absent = registered.clone();
        absent.address_hrp = None;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000307",
            &registered,
            &absent,
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let mut matching = report.block_summaries[0].clone();
        matching.block_number = 11;
        matching.block_hash = format!("0x{:064x}", 12).parse().unwrap();
        matching.parent_hash = format!("0x{:064x}", 11).parse().unwrap();
        matching.network_identity = registered.clone();
        report.block_summaries.push(matching);
        report.validate().unwrap();

        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        // Both samples must be reported on by the Receipt: a missing entry
        // would let the "no rejection" assertion below pass for the wrong
        // reason, so each height is looked up by presence, not by absence.
        let sample_at = |height: u64| {
            receipt
                .samples
                .iter()
                .find(|sample| {
                    sample.sample == platpulse_core::receipt::SampleRef::Block { height }
                })
                .unwrap_or_else(|| panic!("the Receipt reports the sample for height {height}"))
                .clone()
        };
        assert_eq!(receipt.samples.len(), 2);

        let refused = sample_at(10);
        assert_eq!(refused.disposition, SampleDispositionKind::TerminalRejected);
        let absent = refused
            .rejection
            .expect("the HRP-less stamp is contradicted");
        assert_eq!(
            absent.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        // The refusal names the missing HRP without pretending it was this
        // Network's registered value.
        assert!(
            absent
                .reason
                .contains("address_hrp stamped not reported registered unset"),
            "{}",
            absent.reason
        );
        // The sibling stamp that genuinely matches the Registry is accepted on
        // its own merits, not condemned through a key it shares with a
        // normalized spelling of the absent HRP.
        let matched = sample_at(11);
        assert_eq!(matched.disposition, SampleDispositionKind::Accepted);
        assert!(
            matched.rejection.is_none(),
            "the matching stamp is accepted, not judged by its sibling"
        );

        let accepted: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id = $1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(accepted, 1);
        // The data plane, the Receipt and the Server's own gap row agree: the
        // accepted summary is counted once and only the contradicted height is
        // refused.
        let counted: i64 = sqlx::query_scalar(
            "SELECT cumulative_block_count FROM block_history_state WHERE node_id = $1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(counted, 1);
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!((gaps[0].0, gaps[0].1), (10, 10));
        assert_eq!(gaps[0].2, absent.reason);
    }

    /// A contradicted stamp is the more specific finding about a sample, so it
    /// outranks the generic refusals in the Receipt: a GapBackfill sample
    /// outside an open recoverable gap reports the identity contradiction the
    /// Server records on its History Gap, instead of two surfaces naming two
    /// different causes. A sample whose stamp matches the Registry keeps the
    /// backfill-protocol rejection it always had.
    #[tokio::test]
    pub(super) async fn identity_refusal_agrees_with_the_gap_it_records() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut mismatched = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000306",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = mismatched.inventory.nodes[0].node_id;
        mismatched.block_summaries[0].source = platpulse_core::block::BlockSource::GapBackfill;
        mismatched.validate().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&mismatched).unwrap()).await;
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection.reason.contains("genesis_hash stamped 0xaaaa"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);

        // The same sample shape with a matching stamp is new history above the
        // High-Water Mark, which a backfill may supply (issue #281), and the
        // Server records no identity gap for it.
        let mut matching = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000307",
            &registered_identity(),
            &registered_identity(),
            &[20],
        );
        matching.report_sequence = mismatched.report_sequence + 1;
        matching.block_summaries[0].source = platpulse_core::block::BlockSource::GapBackfill;
        matching.validate().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&matching).unwrap()).await;
        assert!(receipt.samples[0].rejection.is_none());
        assert_eq!(recorded_gap_reasons(&state, node_id).await, gaps);
    }

    /// History Gap rows carry their author: the Server writes its own refusals
    /// as 'server', and a gap an Agent declares about itself stays 'agent'
    /// evidence. Only the Server's own rows may be presented as its live
    /// refusal diagnosis, so a reporting Agent cannot fabricate one
    /// (AGENTS.md: the Server is the trust boundary).
    #[tokio::test]
    pub(super) async fn history_gaps_carry_their_author() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000308",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let server_reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(10, 10, server_reason.clone(), "server".to_owned())]
        );

        // The Agent applies the Receipt and reports the same gap back with its
        // own wording, plus a gap of its own that the Server never refused. The
        // Server's row keeps its text and its author, and the Agent's row is
        // stored as the Agent's.
        let mut declared = report.clone();
        declared.report_sequence += 1;
        declared.report_id = "0195f2a1-0013-4013-8013-000000000309".parse().unwrap();
        declared.block_summaries.clear();
        declared.history_gaps = [10u64, 12]
            .into_iter()
            .map(|height| platpulse_core::gap::HistoryGap {
                node_id,
                kind: platpulse_core::gap::GapKind::ServerRejected,
                from_height: height,
                to_height: height,
                reason: "locally retained history gap".to_owned(),
                recorded_at: report.generated_at,
            })
            .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![
                (10, 10, server_reason, "server".to_owned()),
                (
                    12,
                    12,
                    "locally retained history gap".to_owned(),
                    "agent".to_owned()
                ),
            ]
        );
    }

    /// A refusal is the Server's own finding, so it must correct a row the
    /// Agent declared first for the same height: the Agent's insert keeps its
    /// hands off an existing row, and the Server's upsert owns the row it
    /// refused. Otherwise a reporting Agent could dictate what the Admin
    /// diagnosis says about a refusal the Server made (AGENTS.md: the Server
    /// is the trust boundary).
    #[tokio::test]
    pub(super) async fn a_server_refusal_corrects_the_gap_the_agent_declared_first() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut declared = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000310",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[],
        );
        let node_id = declared.inventory.nodes[0].node_id;
        declared.history_gaps = std::iter::once(platpulse_core::gap::HistoryGap {
            node_id,
            kind: platpulse_core::gap::GapKind::ServerRejected,
            from_height: 10,
            to_height: 10,
            reason: "locally retained history gap".to_owned(),
            recorded_at: declared.generated_at,
        })
        .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(
                10,
                10,
                "locally retained history gap".to_owned(),
                "agent".to_owned()
            )]
        );

        let mut refused = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000311",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        refused.report_sequence = declared.report_sequence + 1;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&refused).unwrap()).await;
        let server_reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(10, 10, server_reason, "server".to_owned())]
        );
        // The Server also stamps the row with its own clock instead of keeping
        // the Agent's claim about when the refusal happened. The Receipt's
        // `server_time` and the row's `created_at` are both canonical
        // second-precision UTC text, and the row's text is minted first, so it
        // can only sort at or before the Receipt's time — never at the Agent's
        // past-dated `recorded_at`.
        let (created_at, authored_by): (String, String) = sqlx::query_as(
            "SELECT created_at, authored_by FROM block_history_gaps WHERE node_id = $1 AND from_height = 10 AND to_height = 10",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(authored_by, "server");
        assert_ne!(
            created_at,
            declared.generated_at.to_string(),
            "the Agent's own timestamp must not survive the Server's refusal"
        );
        assert!(created_at.as_str() <= receipt.server_time.to_string().as_str());
    }

    /// Acceptance evidence for the Admin refusal diagnosis outlives raw Block
    /// Summary retention: the Server stamps the moment it last accepted a
    /// summary on the durable per-Node history state (ADR 0011 decision 4).
    #[tokio::test]
    pub(super) async fn accepted_history_records_when_it_resumed() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000310",
            &registered_identity(),
            &registered_identity(),
            &[20, 21],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);

        let accepted: String = sqlx::query_scalar(
            "SELECT accepted_at FROM block_summaries WHERE node_id=$1 ORDER BY block_number DESC LIMIT 1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let recorded: Option<String> = sqlx::query_scalar(
            "SELECT last_accepted_summary_at FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            recorded,
            Some(accepted),
            "the acceptance stamp is the same Server-authored text as the summary it proves"
        );
    }

    /// A Node that really serves another Network is not a stale stamp: no
    /// Agent restart can reconcile it, and the reason must say so instead of
    /// sending an operator through the restart runbook.
    #[tokio::test]
    pub(super) async fn cross_network_conflict_is_not_reported_as_a_stale_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000303",
            &pre_upgrade_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection.reason.contains(
                "genuine cross-Network conflict that restarting the Agent cannot reconcile"
            ),
            "{}",
            rejection.reason
        );
        assert!(
            !rejection.reason.contains("is the only remedy"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);
    }

    /// Without a complete successful identity observation the Server cannot
    /// tell a stale stamp from a cross-Network conflict, and the reason must
    /// say that rather than guess.
    #[tokio::test]
    pub(super) async fn unobserved_identity_is_not_reported_as_a_stale_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000304",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        report.nodes[0].chain.network_identity.status = ComponentStatus::Error;
        report.nodes[0].chain.network_identity.latest = None;
        report.nodes[0].chain.network_identity.latest_observed_at = None;
        report.nodes[0].chain.network_identity.error =
            Some(platpulse_core::component::BoundedError {
                code: "rpc_unreachable".into(),
                message: "identity probe failed".into(),
            });
        report.validate().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection
                .reason
                .contains("no complete successful Network Identity observation"),
            "{}",
            rejection.reason
        );
        assert!(
            rejection.reason.contains(
                "Confirm the Node's current identity and then restart the Agent on that Host"
            ),
            "{}",
            rejection.reason
        );
        assert!(
            !rejection.reason.contains("is the only remedy"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);
    }
}
