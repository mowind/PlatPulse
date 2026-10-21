//! Agent diagnostics egress (issue #250): leveled, structured, redacted.

use std::io::Write;
use std::sync::{Arc, Mutex};

use platpulse_agent::logging::{LogConfig, LogFormat, subscriber};

const T: &str = "platpulse_agent::cli";

struct SinkWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture(format: LogFormat, rust_log: Option<&str>, emit: impl FnOnce()) -> Vec<String> {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let writer = {
        let sink = Arc::clone(&sink);
        move || SinkWriter(Arc::clone(&sink))
    };
    let config = LogConfig {
        level: None,
        format,
    };
    let (subscriber, _) = subscriber(&config, rust_log, writer);
    tracing::subscriber::with_default(subscriber, emit);
    let bytes = sink.lock().unwrap().clone();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn rpc_and_server_credentials_never_reach_the_writer() {
    for format in [LogFormat::Text, LogFormat::Json] {
        let lines = capture(format, None, || {
            tracing::warn!(
                target: T,
                error = "dial wss://operator:hunter2@node.example:8546/ws failed",
                credential = "pp_agent_credential_secret",
                "Agent report delivery deferred: authorization=Bearer bearer-secret"
            );
        });
        assert_eq!(lines.len(), 1, "{lines:?}");
        let output = &lines[0];
        for secret in ["hunter2", "pp_agent_credential_secret", "bearer-secret"] {
            assert!(
                !output.contains(secret),
                "{format:?} leaked {secret}: {output}"
            );
        }
        assert!(
            output.contains("node.example"),
            "host stays diagnosable: {output}"
        );
    }
}

#[test]
fn json_lines_carry_node_scope_and_report_identity() {
    let lines = capture(LogFormat::Json, None, || {
        let span = tracing::error_span!("node", node_id = "node-1");
        let _entered = span.enter();
        tracing::warn!(target: T, report_id = "r-1", report_sequence = 7_u64, "collection deferred");
    });
    let event: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(event["level"], "WARN");
    assert_eq!(event["node_id"], "node-1");
    assert_eq!(event["report_id"], "r-1");
    assert_eq!(event["report_sequence"], 7);
}

#[test]
fn default_is_info_and_rust_log_debug_opens_detail() {
    let quiet = capture(
        LogFormat::Text,
        None,
        || tracing::debug!(target: T, "detail"),
    );
    assert!(quiet.is_empty(), "{quiet:?}");
    let verbose = capture(
        LogFormat::Text,
        Some("debug"),
        || tracing::debug!(target: T, "detail"),
    );
    assert_eq!(verbose.len(), 1);
}
