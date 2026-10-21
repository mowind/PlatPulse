//! Process diagnostics (issue #250): leveled, structured, redacted at the exit.
//!
//! Every Server diagnostic goes through `tracing` and leaves the process through
//! the single egress built here. The egress is the redaction boundary: string
//! field values and the message pass through [`crate::redaction`] and a field
//! whose *name* is a credential key is replaced wholesale, so a new log line is
//! redacted without its author remembering to call anything. Numbers and
//! booleans carry no secret and stay typed.
//!
//! Field names come from the single vocabulary in docs/design/logging.md ("字段词表")
//! (aligned with `CONTEXT.md`); `scripts/check-logging.py` enforces it.
//!
//! stdout is reserved for machine-readable and CLI result output; diagnostics
//! and lifecycle lines go to stderr (the journal under systemd). Writes happen
//! on a dedicated thread behind a bounded queue so a stalled journal never
//! blocks the async runtime; when the queue is full a line is dropped rather
//! than waited for.

use std::fmt::Write as _;
use std::io::Write as _;

use clap::{Args, ValueEnum};
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// Directive used for the Server's own targets when neither `--log-level` nor
/// `RUST_LOG` is given.
const DEFAULT_LEVEL: LevelFilter = LevelFilter::INFO;

/// Targets that belong to PlatPulse. Third-party crates stay at `warn` unless
/// the operator opens them with `RUST_LOG` (SQLx logs every statement at
/// `info`, which would drown the journal).
const OWN_TARGETS: [&str; 1] = ["platpulse_server"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    /// `ts LEVEL target message key=value …` on one line (journald friendly).
    Text,
    /// One JSON object per line.
    Json,
}

/// Logging flags shared by every subcommand.
#[derive(Debug, Clone, Args)]
pub struct LogArgs {
    /// Log level for PlatPulse targets (error, warn, info, debug, trace).
    /// Overrides `RUST_LOG`. Default: info.
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<LevelFilter>,
    /// Diagnostic line format on stderr.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Text)]
    pub log_format: LogFormat,
}

impl From<&LogArgs> for LogConfig {
    fn from(args: &LogArgs) -> Self {
        Self {
            level: args.log_level,
            format: args.log_format,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LogConfig {
    pub level: Option<LevelFilter>,
    pub format: LogFormat,
}

/// The background writer's guard, held for the process lifetime and drained by
/// [`flush`] (same shape as the Agent, which has `process::exit` paths).
static WORKER: std::sync::Mutex<Option<tracing_appender::non_blocking::WorkerGuard>> =
    std::sync::Mutex::new(None);

/// Drain queued diagnostics and stop the writer. Call right before the
/// process exits; later lines are dropped.
pub fn flush() {
    let guard = WORKER.lock().ok().and_then(|mut worker| worker.take());
    drop(guard);
}

/// Install the global subscriber writing to stderr through a non-blocking
/// queue. Safe to call once per process; a second call keeps the first.
pub fn init(config: &LogConfig) {
    let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(true)
        .thread_name("platpulse-log")
        .finish(std::io::stderr());
    let rust_log = std::env::var("RUST_LOG").ok();
    let (subscriber, warning) = subscriber(config, rust_log.as_deref(), writer);
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        return;
    }
    if let Ok(mut worker) = WORKER.lock() {
        *worker = Some(guard);
    }
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
}

/// Build the subscriber for `config`. The second value reports an unusable
/// `RUST_LOG` that was ignored; the caller logs it once the subscriber is live.
pub fn subscriber<W>(
    config: &LogConfig,
    rust_log: Option<&str>,
    writer: W,
) -> (impl Subscriber + Send + Sync + 'static, Option<String>)
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let (filter, warning) = resolve_filter(config.level, rust_log);
    let egress = Egress {
        writer,
        format: config.format,
    };
    (
        tracing_subscriber::registry().with(filter).with(egress),
        warning,
    )
}

fn resolve_filter(
    level: Option<LevelFilter>,
    rust_log: Option<&str>,
) -> (EnvFilter, Option<String>) {
    if let Some(level) = level {
        return (own_targets_at(level), None);
    }
    match rust_log.map(str::trim).filter(|value| !value.is_empty()) {
        None => (own_targets_at(DEFAULT_LEVEL), None),
        Some(directives) => match EnvFilter::try_new(directives) {
            Ok(filter) => (filter, None),
            Err(_) => (
                own_targets_at(DEFAULT_LEVEL),
                Some("RUST_LOG is not a valid filter; using the default level (info)".to_owned()),
            ),
        },
    }
}

fn own_targets_at(level: LevelFilter) -> EnvFilter {
    let mut directives = String::from("warn");
    for target in OWN_TARGETS {
        let _ = write!(directives, ",{target}={level}");
    }
    EnvFilter::new(directives)
}

/// A recorded field value. Strings are redacted when rendered; numbers and
/// booleans stay typed.
#[derive(Debug, Clone)]
enum Value {
    Text(String),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Bool(bool),
}

#[derive(Default)]
struct Fields {
    message: Option<String>,
    values: Vec<(&'static str, Value)>,
}

impl Fields {
    fn push(&mut self, field: &Field, value: Value) {
        if field.name() == "message" {
            if let Value::Text(text) = value {
                self.message = Some(text);
            }
            return;
        }
        let name = field.name();
        self.values.retain(|(existing, _)| *existing != name);
        self.values.push((name, value));
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, Value::Text(value.to_owned()));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, Value::Signed(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, Value::Unsigned(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, Value::Float(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, Value::Bool(value));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.push(field, Value::Text(value.to_string()));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.push(field, Value::Text(format!("{value:?}")));
    }
}

/// Span fields captured once at span creation and merged into every event
/// emitted inside the span (this is how `request_id` reaches each line).
struct SpanFields(Vec<(&'static str, Value)>);

struct Egress<W> {
    writer: W,
    format: LogFormat,
}

impl<S, W> Layer<S> for Egress<W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> MakeWriter<'a> + 'static,
{
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        attributes.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanFields(fields.values));
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        values.record(&mut fields);
        if let Some(span) = ctx.span(id)
            && let Some(existing) = span.extensions_mut().get_mut::<SpanFields>()
        {
            for (name, value) in fields.values {
                existing.0.retain(|(current, _)| *current != name);
                existing.0.push((name, value));
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut own = Fields::default();
        event.record(&mut own);

        let mut merged: Vec<(&'static str, Value)> = Vec::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(fields) = span.extensions().get::<SpanFields>() {
                    for (name, value) in &fields.0 {
                        merged.retain(|(existing, _)| existing != name);
                        merged.push((name, value.clone()));
                    }
                }
            }
        }
        for (name, value) in own.values {
            merged.retain(|(existing, _)| *existing != name);
            merged.push((name, value));
        }

        let metadata = event.metadata();
        let line = render(
            self.format,
            &timestamp(),
            metadata.level(),
            metadata.target(),
            own.message.as_deref().unwrap_or(""),
            &merged,
        );
        // One write per line keeps a line intact when several threads log.
        let _ = self.writer.make_writer().write_all(line.as_bytes());
    }
}

fn timestamp() -> String {
    use time::macros::format_description;
    const FORMAT: &[time::format_description::FormatItem<'static>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    time::OffsetDateTime::now_utc()
        .format(FORMAT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000Z".to_owned())
}

const REDACTED: &str = "[REDACTED]";

fn redacted_text(name: &str, value: &Value) -> Option<String> {
    if crate::redaction::is_sensitive_field(name) {
        return Some(REDACTED.to_owned());
    }
    match value {
        // The listen address is operator configuration, not observed data:
        // credentials are still removed but the address stays readable.
        Value::Text(text) if name == "listen" => Some(crate::redaction::redact_credentials(text)),
        Value::Text(text) => Some(crate::redaction::redact_sensitive(text)),
        _ => None,
    }
}

fn render(
    format: LogFormat,
    ts: &str,
    level: &Level,
    target: &str,
    message: &str,
    fields: &[(&'static str, Value)],
) -> String {
    let message = crate::redaction::redact_sensitive(message);
    match format {
        LogFormat::Text => render_text(ts, level, target, &message, fields),
        LogFormat::Json => render_json(ts, level, target, &message, fields),
    }
}

fn render_text(
    ts: &str,
    level: &Level,
    target: &str,
    message: &str,
    fields: &[(&'static str, Value)],
) -> String {
    let mut line = String::with_capacity(96);
    let _ = write!(line, "{ts} {level:<5} {target} ");
    for character in message.chars() {
        if character.is_control() {
            line.extend(character.escape_default());
        } else {
            line.push(character);
        }
    }
    for (name, value) in fields {
        line.push(' ');
        line.push_str(name);
        line.push('=');
        match redacted_text(name, value) {
            Some(text) => push_text_value(&mut line, &text),
            None => match value {
                Value::Signed(number) => {
                    let _ = write!(line, "{number}");
                }
                Value::Unsigned(number) => {
                    let _ = write!(line, "{number}");
                }
                Value::Float(number) => {
                    let _ = write!(line, "{number}");
                }
                Value::Bool(flag) => {
                    let _ = write!(line, "{flag}");
                }
                Value::Text(_) => unreachable!("text is always rendered through redaction"),
            },
        }
    }
    line.push('\n');
    line
}

fn push_text_value(line: &mut String, text: &str) {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && c != '"' && c != '=' && c != '\\');
    if plain {
        line.push_str(text);
    } else {
        // serde_json's string escaping is a valid, single-line quoting.
        line.push_str(&serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned()));
    }
}

fn render_json(
    ts: &str,
    level: &Level,
    target: &str,
    message: &str,
    fields: &[(&'static str, Value)],
) -> String {
    const ENVELOPE: [&str; 4] = ["ts", "level", "target", "message"];
    let mut line = String::with_capacity(160);
    let json = |text: &str| serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned());
    let _ = write!(
        line,
        "{{\"ts\":{},\"level\":{},\"target\":{},\"message\":{}",
        json(ts),
        json(level.as_str()),
        json(target),
        json(message)
    );
    for (name, value) in fields {
        if ENVELOPE.contains(name) {
            continue;
        }
        let rendered = match redacted_text(name, value) {
            Some(text) => serde_json::Value::String(text),
            None => match value {
                Value::Signed(number) => serde_json::Value::from(*number),
                Value::Unsigned(number) => serde_json::Value::from(*number),
                Value::Float(number) => serde_json::Number::from_f64(*number)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null),
                Value::Bool(flag) => serde_json::Value::Bool(*flag),
                Value::Text(_) => unreachable!("text is always rendered through redaction"),
            },
        };
        let _ = write!(line, ",{}:{}", json(name), rendered);
    }
    line.push_str("}\n");
    line
}
