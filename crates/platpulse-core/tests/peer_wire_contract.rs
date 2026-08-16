use platpulse_core::{AgentReport, WireError};
use serde_json::{Value, json};

fn report_value() -> Value {
    serde_json::from_str(include_str!("fixtures/report_v1_minimal.json")).unwrap()
}

fn peer_component() -> Value {
    json!({
        "status": "ok",
        "attempted_at": "2026-08-12T10:00:00Z",
        "latest_observed_at": "2026-08-12T10:00:00Z",
        "state_revision": 1,
        "value_revision": 1,
        "latest": {
            "peers": [{
                "peer_id": "peer-a",
                "remote_ip": "203.0.113.4",
                "direction": "inbound",
                "trusted": true,
                "static_peer": false,
                "consensus_peer": true,
                "client_name": "PlatON/v1",
                "caps": ["cbft/1", "platon/1"],
                "cbft_protocol_version": 1,
                "cbft_highest_qc_block": 100,
                "cbft_locked_block": 99,
                "cbft_commit_block": 98
            }]
        }
    })
}

#[test]
fn optional_peer_component_is_backward_compatible_and_typed() {
    let mut value = report_value();
    value["nodes"][0]["chain"]["peers"] = peer_component();
    let report: AgentReport = serde_json::from_value(value).unwrap();
    assert_eq!(report.validate(), Ok(()));
    let peer = &report.nodes[0]
        .chain
        .peers
        .as_ref()
        .unwrap()
        .latest
        .as_ref()
        .unwrap()
        .peers[0];
    assert_eq!(peer.peer_id, "peer-a");
    assert_eq!(peer.remote_ip.as_deref(), Some("203.0.113.4"));
    assert!(report_value()["nodes"][0]["chain"].get("peers").is_none());
}

#[test]
fn peer_snapshot_rejects_duplicate_ids_and_non_literal_ips() {
    let mut value = report_value();
    let mut component = peer_component();
    let peers = component["latest"]["peers"].as_array_mut().unwrap();
    peers.push(peers[0].clone());
    value["nodes"][0]["chain"]["peers"] = component;
    let report: AgentReport = serde_json::from_value(value).unwrap();
    assert!(matches!(
        report.validate(),
        Err(WireError::DuplicatePeerId { .. })
    ));

    let mut value = report_value();
    let mut component = peer_component();
    component["latest"]["peers"][0]["remote_ip"] = json!("peer.example");
    value["nodes"][0]["chain"]["peers"] = component;
    let report: AgentReport = serde_json::from_value(value).unwrap();
    assert_eq!(
        report.validate(),
        Err(WireError::ValueOutOfRange {
            field: "peers[].remote_ip"
        })
    );

    let mut value = report_value();
    let mut component = peer_component();
    component["latest"]["peers"][0]["caps"] = json!(["cbft/1", "cbft/1"]);
    value["nodes"][0]["chain"]["peers"] = component;
    let report: AgentReport = serde_json::from_value(value).unwrap();
    assert_eq!(
        report.validate(),
        Err(WireError::ValueOutOfRange {
            field: "peers[].caps"
        })
    );
}

#[test]
fn peer_snapshot_rejects_unknown_fields_and_bounds() {
    let mut value = report_value();
    let mut component = peer_component();
    component["latest"]["peers"][0]["raw_protocols"] = json!({});
    value["nodes"][0]["chain"]["peers"] = component;
    assert!(serde_json::from_value::<AgentReport>(value).is_err());

    let mut value = report_value();
    value["nodes"][0]["chain"]["peers"] = Value::Null;
    assert!(serde_json::from_value::<AgentReport>(value).is_err());

    let mut value = report_value();
    let mut component = peer_component();
    component["latest"]["peers"][0]["direction"] = json!("sideways");
    value["nodes"][0]["chain"]["peers"] = component;
    assert!(serde_json::from_value::<AgentReport>(value).is_err());

    let mut value = report_value();
    let mut component = peer_component();
    component["latest"]["peers"][0]["caps"] = json!(vec!["cbft/1"; 65]);
    value["nodes"][0]["chain"]["peers"] = component;
    let report: AgentReport = serde_json::from_value(value).unwrap();
    assert_eq!(
        report.validate(),
        Err(WireError::TooManyEntries {
            field: "peers[].caps",
            len: 65,
            max: 64
        })
    );

    let mut value = report_value();
    let mut component = peer_component();
    component["latest"]["peers"] = json!(vec![component["latest"]["peers"][0].clone(); 1025]);
    value["nodes"][0]["chain"]["peers"] = component;
    let report: AgentReport = serde_json::from_value(value).unwrap();
    assert_eq!(
        report.validate(),
        Err(WireError::TooManyEntries {
            field: "peers",
            len: 1025,
            max: 1024
        })
    );
}
