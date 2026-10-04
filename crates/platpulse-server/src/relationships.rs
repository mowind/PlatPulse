//! Recorded Node relationship intervals (issue #221, User Stories 60 and 61).
//!
//! A Node's reporting Agent and its Network are both mutable. A completed Node
//! Transfer moves `nodes.agent_id` (see
//! `http::report_ingestion::resolve_node_transfer`), and a later Report may
//! declare another `network_key`. Reading those columns to answer a question
//! about a past window would apply the relation the Node has *now* to a stretch
//! the Server never observed it in, which Story 61 forbids.
//!
//! So the Server records the relations it accepts, from the instant it accepts
//! them, in `node_relationship_intervals`, and every historical answer is built
//! from those intervals. The recording is prospective and never backfilled: an
//! unrecorded stretch stays unknown. Reading it as "no relation", or as the
//! relation the Node has today, is exactly the mistake this module exists to
//! prevent.
//!
//! A Host relation is not stored separately. Host observations are collected
//! once per Agent and Host alert subjects are keyed by the Agent id, so the
//! recorded Agent interval is the Host relation's evidence too: a Node whose
//! reporting Agent changed cannot silently change its Host mid-window.

use sqlx::{FromRow, SqliteConnection, SqlitePool};
use time::{Duration, OffsetDateTime};

use crate::auth::{format_rfc3339, parse_rfc3339};

/// The Node's reporting Agent.
pub const RELATION_AGENT: &str = "agent";
/// The Network the Node's chain identity belongs to.
pub const RELATION_NETWORK: &str = "network";

/// The relation was recorded when the Server first accepted the Node.
pub const ORIGIN_ENROLLMENT: &str = "enrollment";
/// The relation changed because a Node Transfer completed.
pub const ORIGIN_TRANSFER: &str = "transfer";
/// The relation changed because the Node's Network key changed.
pub const ORIGIN_NETWORK_CHANGE: &str = "network_change";

/// The relation kinds the Server records, in presentation order.
pub const RELATION_KINDS: [&str; 2] = [RELATION_AGENT, RELATION_NETWORK];

const INTERVAL_COLUMNS: &str =
    "interval_id, relation_kind, related_key, origin, valid_from, valid_until, recorded_at";

/// One recorded relationship interval, exactly as the Server wrote it.
#[derive(Debug, Clone, FromRow)]
pub struct RelationshipInterval {
    pub interval_id: String,
    pub relation_kind: String,
    /// The Agent id or Network key the interval records.
    pub related_key: String,
    pub origin: String,
    /// RFC 3339 UTC, the instant the Server accepted this relation.
    pub valid_from: String,
    /// RFC 3339 UTC, or `NULL` while the relation still holds.
    pub valid_until: Option<String>,
    pub recorded_at: String,
}

impl RelationshipInterval {
    pub fn valid_from_at(&self) -> Option<OffsetDateTime> {
        parse_rfc3339(&self.valid_from)
    }

    pub fn valid_until_at(&self) -> Option<OffsetDateTime> {
        self.valid_until.as_deref().and_then(parse_rfc3339)
    }

    /// True while the Server records this relation as holding now.
    pub fn open(&self) -> bool {
        self.valid_until.is_none()
    }
}

/// One stretch of a window during which a single relation held.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationSpan {
    pub related_key: String,
    pub origin: String,
    /// Clipped to the window: never earlier than the window's start.
    pub from: OffsetDateTime,
    /// Clipped to the window: never later than the window's end.
    pub to: OffsetDateTime,
    /// True when the recorded interval has no end, i.e. the relation is recorded
    /// as still holding rather than recorded as having stopped.
    pub continues: bool,
}

/// The recorded relation of one kind, derived for one window.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationView {
    pub kind: &'static str,
    /// The relations that held inside the window, oldest first. Their union is
    /// the part of the window the Server can attribute to a recorded relation.
    pub spans: Vec<RelationSpan>,
    /// The stretches of the window no recorded interval covers. These are
    /// unknown, not empty: the Server holds no record of which relation held.
    pub unknown_spans: Vec<(OffsetDateTime, OffsetDateTime)>,
    /// The earliest instant the Server ever recorded a relation of this kind for
    /// the Node, even one outside the window. Nothing before it is recorded.
    pub recorded_from: Option<OffsetDateTime>,
    /// Whether the Server holds any interval of this kind at all.
    pub recorded: bool,
}

impl RelationView {
    fn derive(
        kind: &'static str,
        rows: &[RelationshipInterval],
        window_from: OffsetDateTime,
        window_to: OffsetDateTime,
    ) -> Self {
        let mut sorted: Vec<&RelationshipInterval> = rows
            .iter()
            .filter(|row| row.relation_kind == kind)
            .collect();
        sorted.sort_by(|left, right| left.valid_from.cmp(&right.valid_from));
        let recorded_from = sorted.iter().filter_map(|row| row.valid_from_at()).min();
        let mut spans: Vec<RelationSpan> = Vec::new();
        for row in &sorted {
            let Some(open) = row.valid_from_at() else {
                continue;
            };
            let until = row.valid_until_at();
            let Some((from, to)) = clipped_span(open, until, window_from, window_to) else {
                continue;
            };
            match spans.last_mut() {
                Some(previous)
                    if previous.related_key == row.related_key && previous.to >= from =>
                {
                    previous.to = previous.to.max(to);
                    previous.continues = previous.continues || until.is_none();
                }
                _ => spans.push(RelationSpan {
                    related_key: row.related_key.clone(),
                    origin: row.origin.clone(),
                    from,
                    to,
                    continues: until.is_none(),
                }),
            }
        }
        let unknown_spans = uncover(&spans, window_from, window_to);
        Self {
            kind,
            spans,
            unknown_spans,
            recorded_from,
            recorded: !sorted.is_empty(),
        }
    }
}

/// The recorded relations of a Node, derived for one window.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedRelations {
    pub agent: RelationView,
    pub network: RelationView,
}

impl RecordedRelations {
    /// Derive the views for one window from the Node's recorded intervals.
    ///
    /// Pure: the caller loads rows, this decides what the window can be
    /// attributed to. Rows outside the window still contribute their
    /// `recorded_from`, so the Reader can say when recording began.
    pub fn derive(
        rows: Vec<RelationshipInterval>,
        window_from: OffsetDateTime,
        window_to: OffsetDateTime,
    ) -> Self {
        Self {
            agent: RelationView::derive(RELATION_AGENT, &rows, window_from, window_to),
            network: RelationView::derive(RELATION_NETWORK, &rows, window_from, window_to),
        }
    }

    pub fn view(&self, kind: &str) -> &RelationView {
        match kind {
            RELATION_NETWORK => &self.network,
            _ => &self.agent,
        }
    }

    /// Every relation recorded anywhere in the window, as (kind, span).
    pub fn spans(&self) -> Vec<(&'static str, &RelationSpan)> {
        let mut spans: Vec<(&'static str, &RelationSpan)> = self
            .agent
            .spans
            .iter()
            .map(|span| (RELATION_AGENT, span))
            .chain(
                self.network
                    .spans
                    .iter()
                    .map(|span| (RELATION_NETWORK, span)),
            )
            .collect();
        spans.sort_by(|left, right| (left.1.from, left.0).cmp(&(right.1.from, right.0)));
        spans
    }
}

/// Record the relations the Server accepts for a Node from `recorded_at` on.
///
/// Called on every accepted Report with the Agent that reported and the Network
/// key it declared, and again inside a completed Node Transfer. The first call
/// opens the intervals; a later call that carries a different Agent or Network
/// closes the open interval at that instant and opens the next one, so the
/// recorded timeline stays contiguous and never overlaps.
///
/// Returns the kinds whose recorded relation changed. A repeated Report that
/// changes nothing writes nothing.
pub async fn record_relations(
    connection: &mut SqliteConnection,
    node_id: &str,
    agent_key: &str,
    network_key: &str,
    recorded_at: &str,
) -> Result<Vec<&'static str>, sqlx::Error> {
    let open = sqlx::query_as::<_, RelationshipInterval>(&format!(
        "SELECT {INTERVAL_COLUMNS} FROM node_relationship_intervals WHERE node_id = ? AND valid_until IS NULL ORDER BY relation_kind, valid_from"
    ))
    .bind(node_id)
    .fetch_all(&mut *connection)
    .await?;

    let mut changed = Vec::new();
    for (kind, key, change_origin) in [
        (RELATION_AGENT, agent_key, ORIGIN_TRANSFER),
        (RELATION_NETWORK, network_key, ORIGIN_NETWORK_CHANGE),
    ] {
        match open.iter().find(|row| row.relation_kind == kind) {
            Some(row) if row.related_key == key => continue,
            Some(row) => {
                let instant = close_instant(&row.valid_from, recorded_at);
                sqlx::query(
                    "UPDATE node_relationship_intervals SET valid_until = ? WHERE interval_id = ?",
                )
                .bind(&instant)
                .bind(&row.interval_id)
                .execute(&mut *connection)
                .await?;
                insert_interval(
                    connection,
                    node_id,
                    kind,
                    key,
                    change_origin,
                    &instant,
                    recorded_at,
                )
                .await?;
            }
            None => {
                insert_interval(
                    connection,
                    node_id,
                    kind,
                    key,
                    ORIGIN_ENROLLMENT,
                    recorded_at,
                    recorded_at,
                )
                .await?;
            }
        }
        changed.push(kind);
    }
    Ok(changed)
}

async fn insert_interval(
    connection: &mut SqliteConnection,
    node_id: &str,
    relation_kind: &str,
    related_key: &str,
    origin: &str,
    valid_from: &str,
    recorded_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO node_relationship_intervals (interval_id, node_id, relation_kind, related_key, origin, valid_from, valid_until, recorded_at) VALUES (?, ?, ?, ?, ?, ?, NULL, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(node_id)
    .bind(relation_kind)
    .bind(related_key)
    .bind(origin)
    .bind(valid_from)
    .bind(recorded_at)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Every recorded interval of a Node, oldest first, including ones outside the
/// caller's window.
pub async fn load_intervals(
    pool: &SqlitePool,
    node_id: &str,
) -> Result<Vec<RelationshipInterval>, sqlx::Error> {
    sqlx::query_as::<_, RelationshipInterval>(&format!(
        "SELECT {INTERVAL_COLUMNS} FROM node_relationship_intervals WHERE node_id = ? ORDER BY valid_from"
    ))
    .bind(node_id)
    .fetch_all(pool)
    .await
}

/// The end of an interval that is being closed at `recorded_at`.
///
/// Timestamps are second-precision RFC 3339, so a change can land in the same
/// second the interval opened; the interval then ends one second after it
/// started rather than collapsing to zero length (the schema requires
/// `valid_until > valid_from`), and the next interval opens at that instant.
/// The tie-break is bounded: it moves the closure of the earlier interval later
/// by one second, never backwards, and it never rewrites a written interval, so
/// the record stays append-only and the Server never back-fills a relation it
/// did not observe. A same-second change therefore costs at most one second of
/// attribution, which is the resolution its own clock records in.
fn close_instant(valid_from: &str, recorded_at: &str) -> String {
    match (parse_rfc3339(valid_from), parse_rfc3339(recorded_at)) {
        (Some(open), Some(at)) if at > open => format_rfc3339(at),
        (Some(open), _) => format_rfc3339(open + Duration::seconds(1)),
        _ => recorded_at.to_string(),
    }
}

/// The stretch of the window an interval covers, if any.
pub fn clipped_span(
    valid_from: OffsetDateTime,
    valid_until: Option<OffsetDateTime>,
    window_from: OffsetDateTime,
    window_to: OffsetDateTime,
) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let from = valid_from.max(window_from);
    let to = valid_until.unwrap_or(window_to).min(window_to);
    if to > from { Some((from, to)) } else { None }
}

/// The stretches of a window the given spans leave uncovered, in order.
fn uncover(
    spans: &[RelationSpan],
    window_from: OffsetDateTime,
    window_to: OffsetDateTime,
) -> Vec<(OffsetDateTime, OffsetDateTime)> {
    let mut covered: Vec<(OffsetDateTime, OffsetDateTime)> =
        spans.iter().map(|span| (span.from, span.to)).collect();
    covered.sort_by_key(|(from, _)| *from);
    let mut gaps = Vec::new();
    let mut cursor = window_from;
    for (from, to) in covered {
        if from > cursor {
            gaps.push((cursor, from));
        }
        cursor = cursor.max(to);
    }
    if window_to > cursor {
        gaps.push((cursor, window_to));
    }
    gaps
}

/// A short human label for a relation kind.
pub fn relation_kind_label(kind: &str) -> &'static str {
    match kind {
        RELATION_NETWORK => "Network",
        RELATION_AGENT => "Reporting Agent",
        _ => "Related subject",
    }
}

/// How a related subject is named inside a Reader sentence.
pub fn related_subject_label(kind: &str) -> &'static str {
    match kind {
        RELATION_NETWORK => "the Network",
        RELATION_AGENT => "the reporting Agent",
        _ => "the related subject",
    }
}

/// How the Server came to record an interval, in Reader-facing words.
pub fn origin_label(origin: &str) -> &'static str {
    match origin {
        ORIGIN_ENROLLMENT => "recorded when the Server first accepted the Node",
        ORIGIN_TRANSFER => "recorded when a Node Transfer completed",
        ORIGIN_NETWORK_CHANGE => "recorded when the Node's Network key changed",
        _ => "recorded by the Server",
    }
}

/// One sentence describing a recorded span, for the Reader's sentence list.
///
/// The wording is deliberately about the Server's record, never about the chain:
/// an interval says what the Server was told and accepted, and its edges are
/// correspondence times rather than key-change times.
pub fn describe_span(kind: &str, span: &RelationSpan) -> String {
    let subject = related_subject_label(kind);
    let origin = origin_label(&span.origin);
    if span.continues {
        format!(
            "{subject} {} is recorded from {} onwards and still holds at the end of this window ({origin})",
            span.related_key,
            format_rfc3339(span.from)
        )
    } else {
        format!(
            "{subject} {} is recorded from {} to {} ({origin})",
            span.related_key,
            format_rfc3339(span.from),
            format_rfc3339(span.to)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interval(
        kind: &str,
        key: &str,
        from: &str,
        until: Option<&str>,
        origin: &str,
    ) -> RelationshipInterval {
        RelationshipInterval {
            interval_id: uuid::Uuid::new_v4().to_string(),
            relation_kind: kind.to_string(),
            related_key: key.to_string(),
            origin: origin.to_string(),
            valid_from: from.to_string(),
            valid_until: until.map(str::to_string),
            recorded_at: from.to_string(),
        }
    }

    fn at(value: &str) -> OffsetDateTime {
        parse_rfc3339(value).expect("test timestamp parses")
    }

    #[test]
    fn no_recorded_interval_leaves_the_whole_window_unknown() {
        let window_from = at("2026-01-02T00:00:00Z");
        let window_to = at("2026-01-03T00:00:00Z");
        let relations = RecordedRelations::derive(Vec::new(), window_from, window_to);
        assert!(relations.spans().is_empty());
        for view in [&relations.agent, &relations.network] {
            assert!(view.spans.is_empty());
            assert!(!view.recorded);
            assert_eq!(view.recorded_from, None);
            assert_eq!(view.unknown_spans, vec![(window_from, window_to)]);
        }
    }

    #[test]
    fn a_relation_that_changed_leaves_the_earlier_stretch_with_its_old_subject() {
        let window_from = at("2026-01-02T00:00:00Z");
        let window_to = at("2026-01-03T00:00:00Z");
        let rows = vec![
            interval(
                RELATION_AGENT,
                "agent-a",
                "2026-01-01T00:00:00Z",
                Some("2026-01-02T06:00:00Z"),
                ORIGIN_ENROLLMENT,
            ),
            interval(
                RELATION_AGENT,
                "agent-b",
                "2026-01-02T06:00:00Z",
                None,
                ORIGIN_TRANSFER,
            ),
        ];
        let relations = RecordedRelations::derive(rows, window_from, window_to);
        let agent = &relations.agent;
        assert_eq!(agent.spans.len(), 2);
        assert_eq!(agent.spans[0].related_key, "agent-a");
        assert_eq!(agent.spans[0].from, window_from);
        assert_eq!(agent.spans[0].to, at("2026-01-02T06:00:00Z"));
        assert!(!agent.spans[0].continues);
        assert_eq!(agent.spans[1].related_key, "agent-b");
        assert!(agent.spans[1].continues);
        assert_eq!(agent.spans[1].to, window_to);
        assert!(agent.unknown_spans.is_empty());
        // The Network was never recorded, so it stays unknown for the window.
        assert_eq!(
            relations.network.unknown_spans,
            vec![(window_from, window_to)]
        );
    }

    #[test]
    fn recording_that_starts_inside_the_window_leaves_the_earlier_part_unknown() {
        let window_from = at("2026-01-02T00:00:00Z");
        let window_to = at("2026-01-03T00:00:00Z");
        let rows = vec![interval(
            RELATION_NETWORK,
            "net-mainnet",
            "2026-01-02T09:30:00Z",
            None,
            ORIGIN_ENROLLMENT,
        )];
        let relations = RecordedRelations::derive(rows, window_from, window_to);
        assert_eq!(
            relations.network.unknown_spans,
            vec![(window_from, at("2026-01-02T09:30:00Z"))]
        );
        assert_eq!(
            relations.network.recorded_from,
            Some(at("2026-01-02T09:30:00Z"))
        );
        let network = &relations.network;
        assert_eq!(network.spans.len(), 1);
        assert_eq!(network.spans[0].related_key, "net-mainnet");
        assert!(network.spans[0].continues);
        assert_eq!(network.spans[0].from, at("2026-01-02T09:30:00Z"));
    }

    #[test]
    fn adjacent_intervals_with_the_same_subject_merge_into_one_span() {
        let window_from = at("2026-01-02T00:00:00Z");
        let window_to = at("2026-01-03T00:00:00Z");
        let rows = vec![
            interval(
                RELATION_AGENT,
                "agent-a",
                "2026-01-01T00:00:00Z",
                Some("2026-01-02T06:00:00Z"),
                ORIGIN_ENROLLMENT,
            ),
            interval(
                RELATION_AGENT,
                "agent-a",
                "2026-01-02T06:00:00Z",
                None,
                ORIGIN_TRANSFER,
            ),
        ];
        let relations = RecordedRelations::derive(rows, window_from, window_to);
        assert_eq!(relations.agent.spans.len(), 1);
        assert_eq!(relations.agent.spans[0].from, window_from);
        assert_eq!(relations.agent.spans[0].to, window_to);
        assert!(relations.agent.spans[0].continues);
    }

    #[test]
    fn an_interval_outside_the_window_covers_nothing_but_still_dates_recording() {
        let window_from = at("2026-01-02T00:00:00Z");
        let window_to = at("2026-01-03T00:00:00Z");
        let rows = vec![interval(
            RELATION_AGENT,
            "agent-a",
            "2025-12-01T00:00:00Z",
            Some("2025-12-02T00:00:00Z"),
            ORIGIN_ENROLLMENT,
        )];
        let relations = RecordedRelations::derive(rows, window_from, window_to);
        assert!(relations.agent.spans.is_empty());
        assert!(relations.agent.recorded);
        assert_eq!(
            relations.agent.recorded_from,
            Some(at("2025-12-01T00:00:00Z"))
        );
        assert_eq!(
            relations.agent.unknown_spans,
            vec![(window_from, window_to)]
        );
    }

    #[test]
    fn a_change_in_the_same_second_does_not_collapse_the_interval() {
        assert_eq!(
            close_instant("2026-01-02T06:00:00Z", "2026-01-02T06:00:00Z"),
            "2026-01-02T06:00:01Z"
        );
        assert_eq!(
            close_instant("2026-01-02T06:00:00Z", "2026-01-02T07:00:00Z"),
            "2026-01-02T07:00:00Z"
        );
    }

    #[test]
    fn describing_a_span_stays_about_the_server_record() {
        let span = RelationSpan {
            related_key: "agent-b".to_string(),
            origin: ORIGIN_TRANSFER.to_string(),
            from: at("2026-01-02T06:00:00Z"),
            to: at("2026-01-03T00:00:00Z"),
            continues: true,
        };
        let text = describe_span(RELATION_AGENT, &span);
        assert!(text.contains("agent-b"));
        assert!(text.contains("2026-01-02T06:00:00Z"));
        assert!(text.contains("Node Transfer"));
    }
}
