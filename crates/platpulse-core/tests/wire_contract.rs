//! Wire contract tests over the frozen fixtures.
//!
//! These tests are the drift detector for the v1 contract: any semantic
//! change to the wire types (field removal, enum renaming, unit changes,
//! optionality changes, …) must surface here as a failure.

use platpulse_core::{
    AgentCapability, AgentReport, BlockSource, BootTransition, ComponentStatus, GapKind,
    InventoryDisposition, NodeCurrentDisposition, ProtocolProposer, ReceiptDisposition,
    RejectionCode, ReportReceipt, SampleDispositionKind, SealSignerMatch,
};
use serde_json::{Value, json};

const REPORT_FIXTURES: &[(&str, &str)] = &[
    (
        "report_v1_canonical.json",
        include_str!("fixtures/report_v1_canonical.json"),
    ),
    (
        "report_v1_minimal.json",
        include_str!("fixtures/report_v1_minimal.json"),
    ),
    (
        "report_v1_drained.json",
        include_str!("fixtures/report_v1_drained.json"),
    ),
];

const RECEIPT_FIXTURES: &[(&str, &str)] = &[
    (
        "receipt_v1_accepted.json",
        include_str!("fixtures/receipt_v1_accepted.json"),
    ),
    (
        "receipt_v1_partially_accepted.json",
        include_str!("fixtures/receipt_v1_partially_accepted.json"),
    ),
    (
        "receipt_v1_rejected.json",
        include_str!("fixtures/receipt_v1_rejected.json"),
    ),
];

fn fixture(name: &str) -> &'static str {
    REPORT_FIXTURES
        .iter()
        .chain(RECEIPT_FIXTURES)
        .find(|(n, _)| *n == name)
        .map(|(_, content)| *content)
        .expect("unknown fixture name")
}

fn fixture_value(name: &str) -> Value {
    serde_json::from_str(fixture(name)).expect("fixture must be valid JSON")
}

/// The reserialized struct must be semantically identical to the fixture:
/// nothing dropped, nothing invented, every enum spelling preserved.
fn assert_round_trip<T>(name: &str, content: &str)
where
    T: serde::de::DeserializeOwned + serde::Serialize + PartialEq + std::fmt::Debug,
{
    let expected: Value = serde_json::from_str(content).expect("fixture must parse as JSON");
    let parsed: T = serde_json::from_str(content).expect("fixture must deserialize");
    let reserialized: Value =
        serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
    assert_eq!(expected, reserialized, "round-trip drift in {name}");
    // In-memory round-trip must be stable as well.
    let json = serde_json::to_string(&parsed).unwrap();
    let reparsed: T = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, reparsed);
}

#[test]
fn report_fixtures_round_trip() {
    for (name, content) in REPORT_FIXTURES {
        assert_round_trip::<AgentReport>(name, content);
    }
}

#[test]
fn report_fixtures_validate() {
    for (name, content) in REPORT_FIXTURES {
        let report: AgentReport = serde_json::from_str(content).unwrap();
        assert_eq!(report.validate(), Ok(()), "fixture {name} must validate");
    }
}

#[test]
fn receipt_fixtures_round_trip() {
    for (name, content) in RECEIPT_FIXTURES {
        assert_round_trip::<ReportReceipt>(name, content);
    }
}

#[test]
fn receipt_fixtures_validate() {
    for (name, content) in RECEIPT_FIXTURES {
        let receipt: ReportReceipt = serde_json::from_str(content).unwrap();
        assert_eq!(receipt.validate(), Ok(()), "fixture {name} must validate");
    }
}

#[test]
fn agent_report_rejects_server_only_received_at() {
    // Trust boundary: received_at is populated by the Server at commit time
    // and must never be carried by an Agent report.
    let mut value = fixture_value("report_v1_canonical.json");
    value["host"]["cpu_percent"]["received_at"] = json!("2026-08-12T10:00:01Z");
    let report: AgentReport = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        report.validate(),
        Err(platpulse_core::WireError::ComponentCarriesReceivedAt {
            component: "cpu_percent"
        })
    );
}

#[test]
fn active_components_require_attempted_at_and_ok_requires_value() {
    let mut report: AgentReport =
        serde_json::from_str(fixture("report_v1_canonical.json")).unwrap();
    report.host.cpu_percent.attempted_at = None;
    assert_eq!(
        report.validate(),
        Err(platpulse_core::WireError::ComponentAttemptedAtMissing {
            component: "cpu_percent"
        })
    );

    let mut report: AgentReport =
        serde_json::from_str(fixture("report_v1_canonical.json")).unwrap();
    report.host.memory.latest = None;
    report.host.memory.latest_observed_at = None;
    assert_eq!(
        report.validate(),
        Err(platpulse_core::WireError::ComponentOkWithoutValue {
            component: "memory"
        })
    );
}

#[test]
fn numeric_contract_bounds_are_enforced() {
    // CPU outside 0..=100
    let mut value = fixture_value("report_v1_canonical.json");
    value["host"]["cpu_percent"]["latest"] = json!(101.0);
    let report: AgentReport = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        report.validate(),
        Err(platpulse_core::WireError::ValueOutOfRange {
            field: "host.cpu_percent"
        })
    );

    // Negative load
    let mut value = fixture_value("report_v1_canonical.json");
    value["host"]["load"]["latest"]["load1"] = json!(-0.5);
    let report: AgentReport = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        report.validate(),
        Err(platpulse_core::WireError::ValueOutOfRange { field: "host.load" })
    );

    // used_bytes > total_bytes
    let mut value = fixture_value("report_v1_canonical.json");
    value["host"]["memory"]["latest"]["used_bytes"] = json!(99999999999999u64);
    let report: AgentReport = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        report.validate(),
        Err(platpulse_core::WireError::UsedExceedsTotal {
            field: "host.memory.used_bytes"
        })
    );

    // current_block > highest_block
    let mut value = fixture_value("report_v1_canonical.json");
    value["nodes"][0]["chain"]["sync"]["latest"]["current_block"] = json!(200000);
    let report: AgentReport = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        report.validate(),
        Err(platpulse_core::WireError::ValueOutOfRange {
            field: "sync.current_block"
        })
    );
}

#[test]
fn tagged_enum_payload_fields_are_strict() {
    let mut value = fixture_value("report_v1_canonical.json");
    value["inventory"]["nodes"][0]["process"]["bogus"] = json!(1);
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["block_summaries"][0]["attribution"]["protocol_proposer"]["bogus"] = json!(1);
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("receipt_v1_accepted.json");
    value["samples"][0]["sample"]["bogus"] = json!(1);
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());
}

#[test]
fn receipt_arrays_are_required_not_defaulted() {
    // Authoritative empty is `[]`; omitting the array is a parse error.
    let mut value = fixture_value("receipt_v1_accepted.json");
    value.as_object_mut().unwrap().remove("samples");
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());

    let mut value = fixture_value("receipt_v1_accepted.json");
    value.as_object_mut().unwrap().remove("nodes");
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());

    let mut value = fixture_value("receipt_v1_accepted.json");
    value["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("accepted_component_revisions");
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());
}

#[test]
fn receipt_consistency_rules() {
    // Accepted receipt cannot carry a rejected inventory.
    let mut value = fixture_value("receipt_v1_accepted.json");
    value["inventory"] = json!("rejected");
    let receipt: ReportReceipt = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        receipt.validate(),
        Err(platpulse_core::WireError::ReceiptWithRejectedInventory {
            disposition: ReceiptDisposition::Accepted
        })
    );

    // Retryable disposition requires a retryable rejection.
    let mut value = fixture_value("receipt_v1_partially_accepted.json");
    value["samples"][1]["rejection"]["code"] = json!("stale_boot");
    value["samples"][1]["rejection"]["retryable"] = json!(false);
    let receipt: ReportReceipt = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        receipt.validate(),
        Err(
            platpulse_core::WireError::SampleDispositionRetryableMismatch {
                disposition: SampleDispositionKind::RetryableRejected,
                retryable: false
            }
        )
    );

    // Terminal disposition with a retryable code/flag is contradictory.
    let mut value = fixture_value("receipt_v1_partially_accepted.json");
    value["samples"][2]["disposition"] = json!("terminal_rejected");
    value["samples"][2]["rejection"]["code"] = json!("gap_not_open");
    value["samples"][2]["rejection"]["retryable"] = json!(true);
    let receipt: ReportReceipt = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        receipt.validate(),
        Err(
            platpulse_core::WireError::SampleDispositionRetryableMismatch {
                disposition: SampleDispositionKind::TerminalRejected,
                retryable: true
            }
        )
    );
}

#[test]
fn rejected_node_entries_must_be_consistent() {
    let mut value = fixture_value("receipt_v1_partially_accepted.json");
    value["nodes"][1]["accepted_component_revisions"] = json!([
        {"component": "rpc", "state_revision": 1, "value_revision": 1}
    ]);
    let receipt: ReportReceipt = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        receipt.validate(),
        Err(platpulse_core::WireError::NodeRejectedWithAcceptedRevisions)
    );
}

#[test]
fn every_receipt_disposition_round_trips() {
    let accepted: ReportReceipt =
        serde_json::from_str(fixture("receipt_v1_accepted.json")).unwrap();
    assert_eq!(accepted.disposition, ReceiptDisposition::Accepted);
    assert_eq!(accepted.inventory, Some(InventoryDisposition::Accepted));
    assert!(accepted.rejections.is_empty());
    assert!(
        accepted
            .nodes
            .iter()
            .all(|n| n.current == NodeCurrentDisposition::Accepted)
    );
    assert!(
        accepted
            .samples
            .iter()
            .all(|s| s.disposition == SampleDispositionKind::Accepted)
    );
    assert!(accepted.samples.iter().all(|s| s.rejection.is_none()));

    let partial: ReportReceipt =
        serde_json::from_str(fixture("receipt_v1_partially_accepted.json")).unwrap();
    assert_eq!(partial.disposition, ReceiptDisposition::PartiallyAccepted);
    assert_eq!(partial.inventory, Some(InventoryDisposition::Accepted));
    let node_b = partial
        .nodes
        .iter()
        .find(|n| n.current == NodeCurrentDisposition::Rejected)
        .expect("node B must be rejected");
    assert_eq!(node_b.rejections[0].code, RejectionCode::NodeCurrentInvalid);
    assert!(!node_b.rejections[0].retryable);
    let retryable = partial
        .samples
        .iter()
        .find(|s| s.disposition == SampleDispositionKind::RetryableRejected)
        .unwrap();
    let rejection = retryable.rejection.as_ref().unwrap();
    assert_eq!(rejection.code, RejectionCode::GapNotOpen);
    assert!(rejection.retryable);
    let terminal = partial
        .samples
        .iter()
        .find(|s| s.disposition == SampleDispositionKind::TerminalRejected)
        .unwrap();
    assert_eq!(
        terminal.rejection.as_ref().unwrap().code,
        RejectionCode::NetworkIdentityMismatch
    );
    assert!(!terminal.rejection.as_ref().unwrap().retryable);

    let rejected: ReportReceipt =
        serde_json::from_str(fixture("receipt_v1_rejected.json")).unwrap();
    assert_eq!(rejected.disposition, ReceiptDisposition::Rejected);
    assert_eq!(rejected.inventory, None);
    assert_eq!(
        rejected.rejections[0].code,
        RejectionCode::UnsupportedProtocolVersion
    );
    assert!(!rejected.rejections[0].retryable);
    assert!(rejected.nodes.is_empty());
    assert!(rejected.samples.is_empty());
}

#[test]
fn required_fields_may_not_be_omitted() {
    let mut value = fixture_value("report_v1_canonical.json");
    value.as_object_mut().unwrap().remove("agent_epoch");
    assert!(
        serde_json::from_str::<AgentReport>(&value.to_string()).is_err(),
        "omitting agent_epoch must fail"
    );

    let mut value = fixture_value("report_v1_canonical.json");
    value.as_object_mut().unwrap().remove("block_summaries");
    assert!(
        serde_json::from_str::<AgentReport>(&value.to_string()).is_err(),
        "block_summaries must be present (may be an empty array)"
    );

    let mut value = fixture_value("receipt_v1_accepted.json");
    value.as_object_mut().unwrap().remove("server_time");
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());
}

#[test]
fn explicit_null_is_never_allowed() {
    let mut value = fixture_value("report_v1_canonical.json");
    value["previous_boot_id"] = Value::Null;
    assert!(
        serde_json::from_str::<AgentReport>(&value.to_string()).is_err(),
        "explicit null for an optional field must fail"
    );

    let mut value = fixture_value("report_v1_canonical.json");
    value["inventory"]["nodes"][0]["display_name"] = Value::Null;
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["host"]["cpu_percent"]["latest"] = Value::Null;
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("receipt_v1_accepted.json");
    value["rotation_hint"] = Value::Null;
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());
}

#[test]
fn unknown_fields_are_rejected() {
    let mut value = fixture_value("report_v1_canonical.json");
    value["bogus_top_level"] = json!(1);
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["host"]["cpu_percent"]["bogus"] = json!(1);
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("receipt_v1_accepted.json");
    value["bogus_receipt_field"] = json!(1);
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());
}

#[test]
fn unknown_enum_values_are_rejected() {
    let mut value = fixture_value("report_v1_canonical.json");
    value["boot_transition"] = json!("jumping");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["host"]["cpu_percent"]["status"] = json!("healthy");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["block_summaries"][0]["source"] = json!("polling");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["block_summaries"][0]["attribution"]["seal_signer_match"] = json!("match");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["history_gaps"][0]["kind"] = json!("zero_activity");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("receipt_v1_accepted.json");
    value["disposition"] = json!("ok");
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());

    let mut value = fixture_value("receipt_v1_accepted.json");
    value["nodes"][0]["current"] = json!("ignored");
    assert!(serde_json::from_str::<ReportReceipt>(&value.to_string()).is_err());
}

#[test]
fn timestamps_must_be_rfc3339_utc() {
    let mut value = fixture_value("report_v1_canonical.json");
    value["generated_at"] = json!("2026-08-12T10:00:00+02:00");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["generated_at"] = json!("2026-08-12 10:00:00Z");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["generated_at"] = json!(1785002400);
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    // +00:00 and fractional seconds are accepted and canonicalized to Z.
    let mut value = fixture_value("report_v1_canonical.json");
    value["generated_at"] = json!("2026-08-12T10:00:00.250+00:00");
    let report: AgentReport = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(report.generated_at.to_string(), "2026-08-12T10:00:00Z");
}

#[test]
fn block_timestamps_are_unix_milliseconds() {
    let report: AgentReport = serde_json::from_str(fixture("report_v1_canonical.json")).unwrap();
    let block = &report.block_summaries[0];
    assert_eq!(block.block_timestamp_ms, 1_786_528_790_000);
    assert_eq!(block.block_interval_ms, None);
    assert_eq!(report.block_summaries[1].block_interval_ms, Some(2000));
    assert_eq!(
        report.block_summaries[2].block_timestamp_ms,
        1_786_528_794_000
    );
}

#[test]
fn json_integers_are_exact_u64() {
    // 2^53 + 1 is not representable as a JS number but must round-trip
    // exactly on the wire (Agent↔Server JSON integers).
    let report: AgentReport = serde_json::from_str(fixture("report_v1_minimal.json")).unwrap();
    assert_eq!(
        report.nodes[0]
            .chain
            .sync
            .latest
            .as_ref()
            .unwrap()
            .current_block,
        9_007_199_254_740_993
    );
    let reserialized = serde_json::to_string(&report).unwrap();
    assert!(reserialized.contains("9007199254740993"));
}

#[test]
fn revisions_round_trip_exactly() {
    let report: AgentReport = serde_json::from_str(fixture("report_v1_canonical.json")).unwrap();
    let host = &report.host;
    assert_eq!(host.cpu_percent.state_revision, 4);
    assert_eq!(host.cpu_percent.value_revision, 4);
    assert_eq!(host.network_throughput.state_revision, 9);
    assert_eq!(host.network_throughput.value_revision, 6);
    let node_b = &report.nodes[1];
    assert_eq!(node_b.process.state_revision, 1);
    assert_eq!(node_b.process.value_revision, 0);
    assert_eq!(node_b.chain.consensus.status, ComponentStatus::Unsupported);
}

#[test]
fn attribution_evidence_round_trips() {
    let report: AgentReport = serde_json::from_str(fixture("report_v1_canonical.json")).unwrap();
    let blocks = &report.block_summaries;
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].source, BlockSource::GapBackfill);
    assert_eq!(blocks[1].source, BlockSource::Subscription);
    assert_eq!(blocks[2].source, BlockSource::Subscription);

    assert_eq!(
        blocks[0].attribution.seal_signer_match,
        SealSignerMatch::Other
    );
    assert_eq!(
        blocks[1].attribution.seal_signer_match,
        SealSignerMatch::SignerSelf
    );
    assert_eq!(
        blocks[2].attribution.seal_signer_match,
        SealSignerMatch::Unknown
    );
    assert!(matches!(
        &blocks[0].attribution.protocol_proposer,
        ProtocolProposer::Verified { identity } if identity.starts_with("0x3333")
    ));
    assert!(matches!(
        &blocks[1].attribution.protocol_proposer,
        ProtocolProposer::Unknown {}
    ));
    assert_eq!(
        blocks[0].attribution.attribution_reason,
        "seal_signer_other; protocol_proposer_verified"
    );
    assert_eq!(blocks[0].attribution.coinbase.as_str().len(), 42);
    assert!(blocks[2].attribution.seal_signer_key_fingerprint.is_none());
}

#[test]
fn unsupported_protocol_major_is_rejected() {
    let mut value = fixture_value("report_v1_canonical.json");
    value["protocol_version"] = json!(2);
    let report: AgentReport = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(
        report.validate(),
        Err(platpulse_core::WireError::UnsupportedProtocolVersion {
            got: 2,
            supported: 1
        })
    );

    let mut value = fixture_value("report_v1_canonical.json");
    value["protocol_version"] = json!(0);
    let report: AgentReport = serde_json::from_str(&value.to_string()).unwrap();
    assert!(report.validate().is_err());
}

#[test]
fn authoritative_empty_is_not_omission() {
    // Empty arrays are authoritative (capabilities []); omission of the
    // array itself is a parse error (covered by required_fields test).
    let report: AgentReport = serde_json::from_str(fixture("report_v1_minimal.json")).unwrap();
    assert!(report.agent_capabilities.is_empty());
    assert!(report.block_summaries.is_empty());
    assert!(report.history_gaps.is_empty());
    assert_eq!(
        serde_json::to_value(report.agent_capabilities).unwrap(),
        json!([])
    );
}

#[test]
fn zero_is_never_used_for_unknown_state() {
    // A disabled/unsupported component carries value_revision 0 and no
    // latest value; it never fabricates a `0`/`false`/Healthy result.
    let report: AgentReport = serde_json::from_str(fixture("report_v1_canonical.json")).unwrap();
    let node_b = &report.nodes[1];
    assert_eq!(node_b.process.status, ComponentStatus::Disabled);
    assert!(node_b.process.latest.is_none());
    assert_eq!(node_b.process.value_revision, 0);
    assert_eq!(node_b.chain.consensus.status, ComponentStatus::Unsupported);
    assert!(node_b.chain.consensus.latest.is_none());
    // Last-good is retained across a collection failure.
    assert_eq!(node_b.chain.sync.status, ComponentStatus::Error);
    assert!(node_b.chain.sync.latest.is_some());
    assert_eq!(
        node_b.chain.sync.error.as_ref().unwrap().code,
        "sync_stalled"
    );
}

#[test]
fn identity_and_format_validation_on_the_wire() {
    let mut value = fixture_value("report_v1_canonical.json");
    value["agent_id"] = json!("not-a-uuid");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["block_summaries"][0]["block_hash"] = json!("0xABC");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["inventory"]["nodes"][0]["rpc_endpoint"] = json!("http://127.0.0.1:6790");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["inventory"]["nodes"][0]["network_key"] = json!("Platon-Mainnet");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());

    let mut value = fixture_value("report_v1_canonical.json");
    value["block_summaries"][0]["attribution"]["coinbase"] = json!("0x1234");
    assert!(serde_json::from_str::<AgentReport>(&value.to_string()).is_err());
}

#[test]
fn capabilities_and_transitions_are_stable() {
    let report: AgentReport = serde_json::from_str(fixture("report_v1_canonical.json")).unwrap();
    assert_eq!(
        report.agent_capabilities,
        vec![
            AgentCapability::BlockSummary,
            AgentCapability::HistoryGap,
            AgentCapability::ProcessSystemd,
            AgentCapability::ProcessPidFile,
            AgentCapability::ConsensusStatus,
        ]
    );
    assert_eq!(report.boot_transition, BootTransition::Continuing);

    let drained: AgentReport = serde_json::from_str(fixture("report_v1_drained.json")).unwrap();
    assert_eq!(drained.boot_transition, BootTransition::DrainedPrevious);
    assert!(drained.previous_boot_id.is_some());
    assert_ne!(drained.boot_id, drained.previous_boot_id.unwrap());

    let minimal: AgentReport = serde_json::from_str(fixture("report_v1_minimal.json")).unwrap();
    assert_eq!(minimal.boot_transition, BootTransition::Continuing);
    assert!(minimal.previous_boot_id.is_none());
}

#[test]
fn gap_kinds_are_stable() {
    let report: AgentReport = serde_json::from_str(fixture("report_v1_canonical.json")).unwrap();
    assert_eq!(report.history_gaps.len(), 1);
    let gap = &report.history_gaps[0];
    assert_eq!(gap.kind, GapKind::UnrecoverableBackfill);
    assert_eq!((gap.from_height, gap.to_height), (100_038, 100_040));
    assert_eq!(
        serde_json::to_string(&GapKind::ServerRejected).unwrap(),
        "\"server_rejected\""
    );
}

#[test]
fn receipt_component_revisions_use_stable_keys() {
    let receipt: ReportReceipt = serde_json::from_str(fixture("receipt_v1_accepted.json")).unwrap();
    let node_a = &receipt.nodes[0];
    assert_eq!(node_a.accepted_component_revisions.len(), 6);
    assert_eq!(
        serde_json::to_value(node_a.accepted_component_revisions[0].component).unwrap(),
        json!("process")
    );
    assert_eq!(node_a.accepted_component_revisions[1].state_revision, 6);
    assert_eq!(node_a.accepted_component_revisions[2].value_revision, 7);
}
