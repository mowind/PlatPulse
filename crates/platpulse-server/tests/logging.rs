//! Diagnostics egress contract (issue #250): leveled, structured, redacted.
//!
//! The seam is the log egress: events and spans go in through `tracing`, bytes
//! come out of the writer an operator's journal would receive. Nothing here
//! looks at the formatter's internals.

use std::io::Write;
use std::sync::{Arc, Mutex};

use platpulse_server::logging::{LogConfig, LogFormat, subscriber};
use tracing::level_filters::LevelFilter;

/// Events emitted from Server code carry a `platpulse_server::…` target.
const T: &str = "platpulse_server::http";

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

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

impl Sink {
    fn writer(&self) -> impl Fn() -> SinkWriter + Send + Sync + 'static {
        let inner = Arc::clone(&self.0);
        move || SinkWriter(Arc::clone(&inner))
    }
    fn lines(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn capture(
    format: LogFormat,
    level: Option<LevelFilter>,
    rust_log: Option<&str>,
    emit: impl FnOnce(),
) -> Vec<String> {
    let sink = Sink::default();
    let config = LogConfig { level, format };
    let (subscriber, _) = subscriber(&config, rust_log, sink.writer());
    tracing::subscriber::with_default(subscriber, emit);
    sink.lines()
}

#[test]
fn json_mode_emits_one_valid_object_per_event_with_the_envelope_and_span_fields() {
    let lines = capture(LogFormat::Json, None, None, || {
        let span = tracing::error_span!("request", request_id = "req-1", route_group = "agent");
        let _entered = span.enter();
        tracing::info!(target: T, status = 202_u16, duration_ms = 17_u64, outcome = "accepted", "report ingested");
    });

    assert_eq!(lines.len(), 1, "{lines:?}");
    let event: serde_json::Value = serde_json::from_str(&lines[0]).expect("valid JSON");
    assert_eq!(event["level"], "INFO");
    assert_eq!(event["message"], "report ingested");
    assert_eq!(event["target"], "platpulse_server::http");
    assert!(event["ts"].as_str().unwrap().ends_with('Z'));
    assert_eq!(event["request_id"], "req-1");
    assert_eq!(event["route_group"], "agent");
    assert_eq!(event["status"], 202, "numbers stay numbers");
    assert_eq!(event["duration_ms"], 17);
    assert_eq!(event["outcome"], "accepted");
}

#[test]
fn text_mode_emits_one_line_with_stable_key_value_pairs() {
    let lines = capture(LogFormat::Text, None, None, || {
        let span = tracing::error_span!("request", request_id = "req-1");
        let _entered = span.enter();
        tracing::warn!(target: T, status = 503_u16, outcome = "unavailable", "report ingestion failed");
    });

    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert!(line.contains(" WARN "), "{line}");
    assert!(line.contains("report ingestion failed"), "{line}");
    assert!(line.contains(" request_id=req-1"), "{line}");
    assert!(line.contains(" status=503"), "{line}");
    assert!(line.contains(" outcome=unavailable"), "{line}");
}

#[test]
fn a_value_cannot_forge_a_second_log_line() {
    for format in [LogFormat::Text, LogFormat::Json] {
        let lines = capture(format, None, None, || {
            tracing::warn!(target: T, error = "boom\n2026-01-01T00:00:00Z  INFO forged line", "failed");
        });
        assert_eq!(lines.len(), 1, "{format:?}: {lines:?}");
    }
}

#[test]
fn credentials_never_reach_the_writer_in_either_format() {
    for format in [LogFormat::Text, LogFormat::Json] {
        let lines = capture(format, None, None, || {
            tracing::error!(target: T,
                error = "rpc_url=wss://user:hunter2@203.0.113.7:8546/?token=rpc-secret failed",
                cookie = "pp_session=cookie-secret",
                authorization = "Bearer bearer-secret",
                token = "field-name-secret",
                "login failed password=message-secret authorization=Bearer message-bearer"
            );
        });
        let output = lines.join("\n");
        for secret in [
            "hunter2",
            "203.0.113.7",
            "rpc-secret",
            "cookie-secret",
            "bearer-secret",
            "field-name-secret",
            "message-secret",
            "message-bearer",
        ] {
            assert!(
                !output.contains(secret),
                "{format:?} leaked {secret}: {output}"
            );
        }
        if matches!(format, LogFormat::Json) {
            serde_json::from_str::<serde_json::Value>(&lines[0]).expect("still valid JSON");
        }
    }
}

#[test]
fn default_level_is_info_and_quiets_third_party_chatter() {
    let lines = capture(LogFormat::Text, None, None, || {
        tracing::debug!(target: T, "platpulse debug detail");
        tracing::info!(target: T, "platpulse info line");
        tracing::info!(target: "sqlx::query", "SELECT 1");
        tracing::warn!(target: "sqlx::query", "slow statement");
    });
    let output = lines.join("\n");
    assert!(!output.contains("platpulse debug detail"), "{output}");
    assert!(output.contains("platpulse info line"), "{output}");
    assert!(!output.contains("SELECT 1"), "{output}");
    assert!(output.contains("slow statement"), "{output}");
}

#[test]
fn rust_log_opens_finer_diagnostics() {
    let lines = capture(LogFormat::Text, None, Some("debug"), || {
        tracing::debug!(target: T, "platpulse debug detail");
    });
    assert_eq!(lines.len(), 1, "{lines:?}");
}

#[test]
fn log_level_flag_beats_rust_log_and_keeps_the_request_span_attached_to_warnings() {
    let lines = capture(
        LogFormat::Text,
        Some(LevelFilter::WARN),
        Some("debug"),
        || {
            let span = tracing::error_span!("request", request_id = "req-9");
            let _entered = span.enter();
            tracing::info!(target: T, "hidden");
            tracing::warn!(target: T, "shown");
        },
    );
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("shown") && lines[0].contains("request_id=req-9"),
        "{lines:?}"
    );
}

#[test]
fn an_unparsable_rust_log_falls_back_to_the_default_and_says_so() {
    let sink = Sink::default();
    let config = LogConfig {
        level: None,
        format: LogFormat::Text,
    };
    let (subscriber, warning) = subscriber(&config, Some("not[a=valid"), sink.writer());
    tracing::subscriber::with_default(subscriber, || tracing::info!(target: T, "still logging"));
    assert!(warning.expect("a warning is reported").contains("RUST_LOG"));
    assert_eq!(sink.lines().len(), 1);
}
