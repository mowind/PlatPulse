//! v2 (Server-managed Inventory Revision) wire contract tests.
//!
//! These are independent of the frozen v1 golden vectors in `wire_contract.rs`:
//! the v2 declaration fingerprint is revision-excluded and Node-ID-sorted, while
//! the v1 content hash stays revision-inclusive and order-sensitive. The golden
//! fingerprint below was computed from the canonical bytes with an independent
//! SHA-256 implementation, not by calling the Rust code under test.

use platpulse_core::{AgentReport, InventoryDeclaration, ReportReceiptV2};

const REPORT_V2_MINIMAL: &str = include_str!("fixtures/report_v2_minimal.json");
const REPORT_V1_CANONICAL: &str = include_str!("fixtures/report_v1_canonical.json");
const RECEIPT_V2_ACCEPTED: &str = include_str!("fixtures/receipt_v2_accepted.json");
const RECEIPT_V2_REJECTED: &str = include_str!("fixtures/receipt_v2_rejected.json");

/// SHA-256 over
/// `{"declaration_version":2,"nodes":[<canonical fixture nodes sorted by id>]}`.
const GOLDEN_V2_FINGERPRINT: &str =
    "0xd27190dc94800a57f196427b996857ad7582414b535ffe51853d580c8917b0c4";

/// SHA-256 of the v1 canonical fixture's revision-inclusive, order-sensitive
/// Inventory serialization `{"revision":7,"nodes":[...]}`, computed with an
/// independent SHA-256 implementation. A serializer or field-order change that
/// alters the frozen v1 rule surfaces here.
const GOLDEN_V1_INVENTORY_SHA256: &str =
    "0x4ccd54eee9d8da83f182b14f66dad3d16520ded61615aa89e80c17441d22a36c";

fn canonical_v1_report() -> AgentReport {
    serde_json::from_str(REPORT_V1_CANONICAL).expect("v1 canonical fixture parses")
}

fn golden_declaration() -> InventoryDeclaration {
    InventoryDeclaration {
        nodes: canonical_v1_report().inventory.nodes,
    }
}

#[test]
fn v2_report_fixture_round_trips_and_validates() {
    let report: AgentReport<InventoryDeclaration> =
        serde_json::from_str(REPORT_V2_MINIMAL).expect("v2 fixture parses as a v2 report");
    assert_eq!(report.protocol_version, 2);
    assert_eq!(report.validate(), Ok(()));

    let expected: serde_json::Value = serde_json::from_str(REPORT_V2_MINIMAL).unwrap();
    let reserialized: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    assert_eq!(expected, reserialized, "v2 fixture round-trip drift");
}

#[test]
fn frozen_v1_decoder_rejects_a_v2_report_and_vice_versa() {
    assert!(
        serde_json::from_str::<AgentReport>(REPORT_V2_MINIMAL).is_err(),
        "a v2 report must not decode as the frozen v1 type"
    );
    assert!(
        serde_json::from_str::<AgentReport<InventoryDeclaration>>(REPORT_V1_CANONICAL).is_err(),
        "a v1 report must not decode as the v2 type"
    );
    let v2: AgentReport<InventoryDeclaration> = serde_json::from_str(REPORT_V2_MINIMAL).unwrap();
    assert_eq!(v2.validate(), Ok(()));

    // The v1 report validated against the v2 inventory shape has no revision
    // field, so a "revision 0" style check cannot apply to it; what must hold is
    // that the v1 type rejects a v2 protocol major.
    let mut v1: AgentReport = serde_json::from_str(REPORT_V1_CANONICAL).unwrap();
    v1.protocol_version = 2;
    assert!(matches!(
        v1.validate(),
        Err(platpulse_core::WireError::UnsupportedProtocolVersion { got: 2, .. })
    ));
}

#[test]
fn v2_declaration_fingerprint_matches_the_independent_golden_vector() {
    assert_eq!(
        golden_declaration().fingerprint().as_str(),
        GOLDEN_V2_FINGERPRINT
    );
}

#[test]
fn v2_fingerprint_ignores_node_order_and_excludes_revision() {
    let declaration = golden_declaration();
    let mut reordered = declaration.clone();
    reordered.nodes.reverse();
    assert_eq!(
        reordered.fingerprint(),
        declaration.fingerprint(),
        "Node order must not change the canonical declaration fingerprint"
    );

    // There is no revision field to change; the same content always fingerprints
    // the same way, which is what makes A -> B -> A a Server-side decision.
    let mut renamed = declaration.clone();
    renamed.nodes[0].display_name = Some("renamed".to_owned());
    assert_ne!(
        renamed.fingerprint(),
        declaration.fingerprint(),
        "a declared field change must change the fingerprint"
    );

    let mut retargeted = declaration.clone();
    retargeted.nodes[0].process = Some(platpulse_core::ProcessSelector::PidFile {
        path: "/run/platon-a.pid".to_owned(),
    });
    assert_ne!(
        retargeted.fingerprint(),
        declaration.fingerprint(),
        "the process selector is part of the declaration"
    );

    let mut rekeyed = declaration.clone();
    rekeyed.nodes[0].network_key = "platon-testnet".parse().unwrap();
    assert_ne!(
        rekeyed.fingerprint(),
        declaration.fingerprint(),
        "the Network key is part of the declaration"
    );
}

#[test]
fn frozen_v1_hash_stays_revision_inclusive_and_order_sensitive() {
    let base = canonical_v1_report().inventory;
    assert_eq!(
        base.content_sha256().as_str(),
        GOLDEN_V1_INVENTORY_SHA256,
        "the frozen v1 content hash must not drift"
    );
    let mut reordered = base.clone();
    reordered.nodes.reverse();
    assert_ne!(
        base.content_sha256(),
        reordered.content_sha256(),
        "the v1 content hash is order-sensitive"
    );

    let mut bumped = base.clone();
    bumped.revision += 1;
    assert_ne!(
        base.content_sha256(),
        bumped.content_sha256(),
        "the v1 content hash includes revision"
    );

    // The same content under the v2 canonical form is order-insensitive, so the
    // two digests are deliberately different values.
    assert_ne!(
        base.content_sha256().as_str(),
        golden_declaration().fingerprint().as_str()
    );
}

#[test]
fn v2_receipt_fixtures_round_trip_and_validate() {
    for content in [RECEIPT_V2_ACCEPTED, RECEIPT_V2_REJECTED] {
        let receipt: ReportReceiptV2 =
            serde_json::from_str(content).expect("v2 receipt fixture parses");
        assert_eq!(receipt.validate(), Ok(()));
        let expected: serde_json::Value = serde_json::from_str(content).unwrap();
        let reserialized: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&receipt).unwrap()).unwrap();
        assert_eq!(expected, reserialized);
    }
}

#[test]
fn v2_accepted_receipt_requires_a_positive_acceptance_pair() {
    let mut missing: serde_json::Value = serde_json::from_str(RECEIPT_V2_ACCEPTED).unwrap();
    missing["inventory"] = serde_json::json!({ "disposition": "accepted" });
    let receipt: ReportReceiptV2 = serde_json::from_value(missing).unwrap();
    assert!(receipt.validate().is_err());

    let mut zero: serde_json::Value = serde_json::from_str(RECEIPT_V2_ACCEPTED).unwrap();
    zero["inventory"] = serde_json::json!({
        "disposition": "accepted",
        "acceptance": { "revision": 0, "fingerprint": GOLDEN_V2_FINGERPRINT }
    });
    let receipt: ReportReceiptV2 = serde_json::from_value(zero).unwrap();
    assert!(receipt.validate().is_err());
}

#[test]
fn v2_rejected_receipt_cannot_carry_an_accepted_pair() {
    let mut value: serde_json::Value = serde_json::from_str(RECEIPT_V2_REJECTED).unwrap();
    value["inventory"] = serde_json::json!({
        "disposition": "rejected",
        "acceptance": { "revision": 1, "fingerprint": GOLDEN_V2_FINGERPRINT }
    });
    let receipt: ReportReceiptV2 = serde_json::from_value(value).unwrap();
    assert!(receipt.validate().is_err());
}
