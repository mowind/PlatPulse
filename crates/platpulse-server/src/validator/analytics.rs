//! Daily snapshots, monthly aggregates and the configured-local-day trend.

use crate::auth::format_rfc3339;
use crate::database::ServerDatabase;
use chrono::{DateTime, Datelike, TimeZone, Utc};
use chrono_tz::Tz;
use sqlx::{Any, Transaction};
use std::collections::BTreeSet;
use time::OffsetDateTime;

use super::error::ValidatorError;
use super::provider::ValidatorObservation;
use super::refresh::bounded_source;
use super::registry::{get_validator, parse_timestamp};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorDailySnapshotRecord {
    pub snapshot_id: String,
    pub validator_id: String,
    pub timezone: String,
    pub local_date: String,
    pub month_key: String,
    pub sample_at: String,
    pub received_at: String,
    pub provider_timestamp: Option<String>,
    pub source: String,
    pub observation_key: String,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorMonthlyAggregateRecord {
    pub aggregate_id: String,
    pub validator_id: String,
    pub timezone: String,
    pub month_key: String,
    pub snapshot_count: i64,
    pub first_sample_at: String,
    pub last_sample_at: String,
    pub rank_min: Option<i64>,
    pub rank_max: Option<i64>,
    pub rank_last: Option<i64>,
    pub stake_last: Option<String>,
    pub reward_last: Option<String>,
    pub reward_rate_last: Option<String>,
    pub delegator_count_last: Option<i64>,
    pub epoch_last: Option<i64>,
    pub block_count_last: Option<i64>,
    pub updated_at: String,
}

/// Convert the observation's provider time when available, otherwise the
/// Server receipt time, into the configured IANA calendar day and month.
/// Provider time makes delayed observations deterministic across refresh
/// retries; receipt time remains the honest fallback for providers without it.
pub fn analytics_period(
    observation: &ValidatorObservation,
    received_at: &str,
    timezone: &str,
) -> Result<(String, String, String), ValidatorError> {
    let timezone = timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(timezone.to_owned()))?;
    let timestamp = observation
        .provider_timestamp
        .as_deref()
        .unwrap_or(received_at);
    let parsed = parse_timestamp(timestamp)?;
    let utc = DateTime::<Utc>::from_timestamp(parsed.unix_timestamp(), parsed.nanosecond())
        .ok_or_else(|| ValidatorError::InvalidTimezone(timezone.to_string()))?;
    let local = utc.with_timezone(&timezone);
    Ok((
        local.format("%Y-%m-%d").to_string(),
        local.format("%Y-%m").to_string(),
        format_rfc3339(parsed),
    ))
}

pub async fn list_daily_snapshots(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorDailySnapshotRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorDailySnapshotRecord>(
        "SELECT snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count FROM validator_daily_snapshots WHERE validator_id = $1 ORDER BY sample_at DESC, local_date DESC LIMIT $2",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

pub async fn list_monthly_aggregates(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorMonthlyAggregateRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorMonthlyAggregateRecord>(
        "SELECT aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max, rank_last, stake_last, reward_last, reward_rate_last, delegator_count_last, epoch_last, block_count_last, updated_at FROM validator_monthly_aggregates WHERE validator_id = $1 ORDER BY month_key DESC, timezone LIMIT $2",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

/// How far back an unbounded daily-trend request reaches, in configured
/// calendar days, and the widest window one request may span. The window is a
/// hard bound on the days and rows one read touches, so an Owner cannot turn a
/// trend page into an unbounded scan (#219).
pub const TREND_DEFAULT_WINDOW_DAYS: i64 = 90;
pub const TREND_MAX_WINDOW_DAYS: i64 = 730;
pub const TREND_DEFAULT_LIMIT: i64 = 90;
pub const TREND_MAX_LIMIT: i64 = 366;
/// Association intervals one trend answer resolves.
pub const TREND_MAX_ASSOCIATIONS: i64 = 100;

/// A bounded daily-trend read for one Validator in the configured IANA
/// calendar (#219, main design §15.4.1). Every bound is a configured local date
/// or a Server-owned instant: the caller never supplies a UTC bucket boundary
/// and the Server never re-buckets a stored snapshot into another zone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidatorTrendQuery {
    pub validator_id: String,
    /// The configured Validator timezone the stored snapshots were bucketed in.
    pub timezone: String,
    /// Requested window start (RFC3339 instant), inclusive.
    pub from: Option<String>,
    /// Requested window end (RFC3339 instant), inclusive.
    pub to: Option<String>,
    /// Paging cursor: one configured local date; only older days answer.
    pub before: Option<String>,
    pub limit: i64,
}

/// One stored day of the trend, with the UTC instants that local day really
/// covers and the timestamps the bucket was chosen from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorTrendPoint {
    pub local_date: String,
    pub month_key: String,
    /// UTC instant the configured local day starts at, inclusive.
    pub day_start: String,
    /// UTC instant the next configured local day starts at, exclusive. A DST
    /// day is 23 or 25 hours wide here instead of pretending to be 24.
    pub day_end: String,
    /// The instant the bucket was chosen by: the Provider timestamp when the
    /// observation carried one, otherwise the Server receipt time.
    pub sample_at: String,
    pub received_at: String,
    pub provider_timestamp: Option<String>,
    /// "provider" or "receipt": which timestamp decided this calendar day.
    pub sample_time: String,
    /// received_at minus provider_timestamp in whole seconds, measured from
    /// this one row. Unknown, never 0, when the observation carried no Provider
    /// timestamp, because then no delay exists to measure.
    pub delay_seconds: Option<i64>,
    /// The observation is stamped after its receipt: the Provider clock is
    /// ahead of the Server clock.
    pub clock_suspect: bool,
    pub source: String,
    pub observation_key: String,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
}

/// A stretch of configured local days the answer proves holds no snapshot
/// (design §11.4): a trend surface draws it as silence, never as a zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorTrendGap {
    pub from_local_date: String,
    pub to_local_date: String,
    pub days: i64,
}

/// One configured calendar month the answer touches, with its month boundary
/// mapped into the UTC investigation coordinate (#219). The boundary comes from
/// the configured zone, so the month is the calendar month the Operator set
/// and not a silently UTC-aligned one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorTrendMonth {
    pub month_key: String,
    /// UTC instant the month's first configured local day starts at.
    pub month_start: String,
    /// UTC instant the next month's first configured local day starts at.
    pub month_end: String,
    pub observed_days: i64,
    pub first_local_date: Option<String>,
    pub last_local_date: Option<String>,
}

/// One Node association of a Validator that still resolves to a Node row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ValidatorAssociationRecord {
    pub link_id: String,
    pub node_id: String,
    pub origin: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub node_display_name: Option<String>,
    pub node_lifecycle: String,
}

/// One bounded trend answer: the points, the coverage behind them, and the
/// disclosure a surface needs so it never presents silence as zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorTrendPage {
    pub timezone: String,
    /// The UTC coordinate the caller asked for, echoed so a clamped answer can
    /// say what it narrowed.
    pub requested_from: String,
    pub requested_to: String,
    pub requested_from_local_date: String,
    pub requested_to_local_date: String,
    /// The configured local dates this answer really covers. Narrower than the
    /// requested window when the caller paged or the window was clamped.
    pub answered_from_local_date: String,
    pub answered_to_local_date: String,
    /// Local days the requested window spans, and the days this answer covers.
    pub requested_days: i64,
    /// True when the requested window was narrowed to the bounded maximum, so
    /// an answer says what it bounded instead of silently shortening.
    pub clamped: bool,
    pub expected_days: i64,
    pub observed_days: i64,
    pub missing_days: i64,
    pub gaps: Vec<ValidatorTrendGap>,
    pub first_observed_local_date: Option<String>,
    pub last_observed_local_date: Option<String>,
    /// Points in ascending configured local-date order.
    pub points: Vec<ValidatorTrendPoint>,
    pub months: Vec<ValidatorTrendMonth>,
    /// True when the window holds more days than the caller's limit answered.
    pub truncated: bool,
    /// The coordinate to pass back as before for the next, older page.
    pub continuation: Option<String>,
    /// Rows stored for this Validator inside the answered stretch under another
    /// timezone. They are counted here and never merged: a stored bucket keeps
    /// the calendar it was formed in.
    pub foreign_rows: i64,
    pub foreign_timezones: Vec<String>,
    /// Association intervals that still resolve to a Node row.
    pub associations: Vec<ValidatorAssociationRecord>,
    pub associations_truncated: bool,
    /// Nodes of this Validator's Network that Purge already deleted. Their
    /// association rows went with them, so those intervals are unavailable
    /// here, never reported as never having existed (Story 68).
    pub deleted_nodes: i64,
    pub association_history_partial: bool,
}

fn local_date_error(value: &str) -> ValidatorError {
    ValidatorError::InvalidTrendWindow(format!("local date must be YYYY-MM-DD: {value}"))
}

fn parse_local_date(value: &str) -> Result<chrono::NaiveDate, ValidatorError> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| local_date_error(value))
}

/// UTC instant a configured local calendar date starts at, in an IANA zone.
///
/// A local day is not a fixed UTC stretch: daylight saving can make it 23 or 25
/// hours, and a spring-forward that crosses midnight (Santiago, Beirut) removes
/// local midnight itself. Such a day starts at the first instant that really
/// exists, so a boundary is never silently moved onto the previous UTC day and
/// no snapshot is ever re-bucketed into a neighbouring calendar date.
fn local_midnight(zone: &Tz, date: chrono::NaiveDate) -> Result<OffsetDateTime, ValidatorError> {
    for hour in 0..=2u32 {
        let Some(naive) = date.and_hms_opt(hour, 0, 0) else {
            continue;
        };
        match zone.from_local_datetime(&naive) {
            // The earliest of an ambiguous pair starts the longer day; this is
            // also the instant the Server bucket rule would use.
            chrono::LocalResult::Single(value) | chrono::LocalResult::Ambiguous(value, _) => {
                return OffsetDateTime::from_unix_timestamp(value.timestamp()).map_err(|_| {
                    ValidatorError::InvalidTrendWindow(format!(
                        "local date {date} is outside the supported range"
                    ))
                });
            }
            chrono::LocalResult::None => continue,
        }
    }
    Err(ValidatorError::InvalidTrendWindow(format!(
        "configured timezone has no local midnight for {date}"
    )))
}

fn local_date_naive(
    zone: &Tz,
    instant: OffsetDateTime,
) -> Result<chrono::NaiveDate, ValidatorError> {
    let utc = DateTime::<Utc>::from_timestamp(instant.unix_timestamp(), instant.nanosecond())
        .ok_or_else(|| {
            ValidatorError::InvalidTrendWindow("instant is outside the supported range".to_owned())
        })?;
    Ok(utc.with_timezone(zone).date_naive())
}

/// UTC instants that bound one configured local calendar date: the first
/// instant inside it (inclusive) and the first instant of the next local day
/// (exclusive). The pair is what maps a retained calendar bucket back onto the
/// UTC investigation coordinate (#219).
pub fn local_day_bounds(
    timezone: &str,
    local_date: &str,
) -> Result<(String, String), ValidatorError> {
    let zone = timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(timezone.to_owned()))?;
    let date = parse_local_date(local_date)?;
    let start = local_midnight(&zone, date)?;
    let next = date.succ_opt().ok_or_else(|| {
        ValidatorError::InvalidTrendWindow(format!("local date {local_date} has no next day"))
    })?;
    let end = local_midnight(&zone, next)?;
    Ok((format_rfc3339(start), format_rfc3339(end)))
}

/// The configured local calendar date one instant falls in.
pub fn local_date_at(timezone: &str, instant: OffsetDateTime) -> Result<String, ValidatorError> {
    let zone = timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(timezone.to_owned()))?;
    Ok(local_date_naive(&zone, instant)?
        .format("%Y-%m-%d")
        .to_string())
}

/// The configured local day and the month it belongs to for one instant, as
/// the stored labels (`2026-02-01` and `2026-02`). Both labels come from the
/// same calendar conversion, so a stored day can never be bucketed into a
/// month its own local date disagrees with.
pub fn local_period_at(
    timezone: &str,
    instant: OffsetDateTime,
) -> Result<(String, String), ValidatorError> {
    let zone = timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(timezone.to_owned()))?;
    let date = local_date_naive(&zone, instant)?;
    Ok((
        date.format("%Y-%m-%d").to_string(),
        date.format("%Y-%m").to_string(),
    ))
}

/// Provider delay for one stored row, or None when the observation carried no
/// usable Provider timestamp. Never 0 for an unknown delay.
fn observed_delay_seconds(received_at: &str, provider_timestamp: &str) -> Option<i64> {
    let received = DateTime::parse_from_rfc3339(received_at).ok()?;
    let stamped = DateTime::parse_from_rfc3339(provider_timestamp).ok()?;
    Some((received - stamped).num_seconds())
}

/// Node associations of one Validator that still resolve to a Node row.
///
/// Node Purge deletes the purged Node's Link rows in the same transaction that
/// removes the Node, so a purged association cannot be listed here. It is
/// reported as unavailable through deleted_nodes and association_history_partial
/// instead, and a retained Validator snapshot history is never dropped, never
/// re-attached to a surviving Node, and never duplicated per Node.
pub async fn list_validator_associations(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorAssociationRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorAssociationRecord>(
        "SELECT l.link_id, l.node_id, l.origin, l.valid_from, l.valid_until, n.display_name AS node_display_name, n.lifecycle AS node_lifecycle FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.validator_id = $1 ORDER BY l.valid_from DESC, l.link_id DESC LIMIT $2",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

/// Nodes of one Network that Purge already deleted, the durable trace of the
/// associations a trend answer can no longer resolve (Story 68).
pub async fn count_deleted_nodes(
    db: &ServerDatabase,
    network_key: &str,
) -> Result<i64, ValidatorError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deleted_nodes WHERE network_key = $1")
            .bind(network_key)
            .fetch_one(db.pool())
            .await?,
    )
}

/// Load one bounded daily-trend page (issue #219).
///
/// The window is expressed as the configured local calendar, uses the same
/// Provider-preferred sample time and tie rule the snapshot writer used, and
/// discloses what it could not answer: days without a snapshot become explicit
/// gaps, foreign-timezone rows are counted instead of merged, and a truncated
/// answer names the cursor for the next older page.
pub async fn load_daily_trend(
    db: &ServerDatabase,
    query: &ValidatorTrendQuery,
    now: OffsetDateTime,
) -> Result<ValidatorTrendPage, ValidatorError> {
    let zone = query
        .timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(query.timezone.clone()))?;
    let requested_to = match query.to.as_deref() {
        Some(value) => parse_timestamp(value)?,
        None => now,
    };
    let requested_from = match query.from.as_deref() {
        Some(value) => parse_timestamp(value)?,
        None => requested_to - time::Duration::days(TREND_DEFAULT_WINDOW_DAYS),
    };
    if requested_from > requested_to {
        return Err(ValidatorError::InvalidTrendWindow(
            "from must not be later than to".to_owned(),
        ));
    }
    let requested_from_date = local_date_naive(&zone, requested_from)?;
    let requested_to_date = local_date_naive(&zone, requested_to)?;
    let requested_days = (requested_to_date - requested_from_date).num_days() + 1;
    // An exclusive cursor: asking for the days older than one local date never
    // re-answers the day the caller already holds.
    let to_date = match query.before.as_deref() {
        Some(value) => parse_local_date(value)?
            .pred_opt()
            .ok_or_else(|| local_date_error(value))?
            .min(requested_to_date),
        None => requested_to_date,
    };
    let mut from_date = requested_from_date;
    let mut clamped = false;
    if to_date >= from_date && (to_date - from_date).num_days() >= TREND_MAX_WINDOW_DAYS {
        from_date = to_date
            .checked_sub_days(chrono::Days::new((TREND_MAX_WINDOW_DAYS - 1) as u64))
            .ok_or_else(|| {
                ValidatorError::InvalidTrendWindow("trend window start is out of range".to_owned())
            })?;
        clamped = true;
    }
    let limit = if query.limit <= 0 {
        TREND_DEFAULT_LIMIT
    } else {
        query.limit.min(TREND_MAX_LIMIT)
    };
    let mut rows: Vec<ValidatorDailySnapshotRecord> = if to_date < from_date {
        Vec::new()
    } else {
        sqlx::query_as::<_, ValidatorDailySnapshotRecord>(
            "SELECT snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count FROM validator_daily_snapshots WHERE validator_id = $1 AND timezone = $2 AND local_date >= $3 AND local_date <= $4 ORDER BY local_date DESC LIMIT $5",
        )
        .bind(&query.validator_id)
        .bind(&query.timezone)
        .bind(from_date.format("%Y-%m-%d").to_string())
        .bind(to_date.format("%Y-%m-%d").to_string())
        .bind(limit + 1)
        .fetch_all(db.pool())
        .await?
    };
    let truncated = rows.len() as i64 > limit;
    if truncated {
        rows.truncate(limit as usize);
    }
    rows.reverse();
    let observed: BTreeSet<chrono::NaiveDate> = rows
        .iter()
        .map(|row| parse_local_date(&row.local_date))
        .collect::<Result<_, _>>()?;
    let oldest_date = rows.first().map(|row| row.local_date.clone());
    let answered_from_date = if truncated {
        oldest_date
            .as_deref()
            .map(parse_local_date)
            .transpose()?
            .unwrap_or(from_date)
    } else {
        from_date
    };
    let answered_days = if to_date < from_date {
        0
    } else {
        (to_date - answered_from_date).num_days() + 1
    };
    let mut points = Vec::with_capacity(rows.len());
    for row in &rows {
        let (day_start, day_end) = local_day_bounds(&query.timezone, &row.local_date)?;
        let delay_seconds = row
            .provider_timestamp
            .as_deref()
            .and_then(|stamped| observed_delay_seconds(&row.received_at, stamped));
        points.push(ValidatorTrendPoint {
            local_date: row.local_date.clone(),
            month_key: row.month_key.clone(),
            day_start,
            day_end,
            sample_at: row.sample_at.clone(),
            received_at: row.received_at.clone(),
            provider_timestamp: row.provider_timestamp.clone(),
            sample_time: if row.provider_timestamp.is_some() {
                "provider".to_owned()
            } else {
                "receipt".to_owned()
            },
            delay_seconds,
            clock_suspect: delay_seconds.is_some_and(|seconds| seconds < 0),
            source: row.source.clone(),
            observation_key: row.observation_key.clone(),
            rank: row.rank,
            stake_amount: row.stake_amount.clone(),
            reward_amount: row.reward_amount.clone(),
            reward_rate: row.reward_rate.clone(),
            delegator_count: row.delegator_count,
            epoch: row.epoch,
            block_count: row.block_count,
        });
    }
    let mut gaps: Vec<ValidatorTrendGap> = Vec::new();
    if answered_days > 0 {
        let mut cursor = answered_from_date;
        while cursor <= to_date {
            if observed.contains(&cursor) {
                let Some(next) = cursor.succ_opt() else { break };
                cursor = next;
                continue;
            }
            let start = cursor;
            let mut last = cursor;
            while let Some(next) = last.succ_opt() {
                if next > to_date || observed.contains(&next) {
                    break;
                }
                last = next;
            }
            gaps.push(ValidatorTrendGap {
                from_local_date: start.format("%Y-%m-%d").to_string(),
                to_local_date: last.format("%Y-%m-%d").to_string(),
                days: (last - start).num_days() + 1,
            });
            let Some(next) = last.succ_opt() else { break };
            cursor = next;
        }
    }
    let mut months: Vec<ValidatorTrendMonth> = Vec::new();
    if answered_days > 0 {
        let mut cursor = answered_from_date;
        while cursor <= to_date {
            let (year, month) = (Datelike::year(&cursor), Datelike::month(&cursor));
            let month_first = chrono::NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(|| {
                ValidatorError::InvalidTrendWindow("month start is out of range".to_owned())
            })?;
            let next_month = if month == 12 {
                chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
            }
            .ok_or_else(|| {
                ValidatorError::InvalidTrendWindow("month end is out of range".to_owned())
            })?;
            let month_last = next_month.pred_opt().ok_or_else(|| {
                ValidatorError::InvalidTrendWindow("month end is out of range".to_owned())
            })?;
            let first = observed.range(month_first..=month_last).next().copied();
            let last = observed
                .range(month_first..=month_last)
                .next_back()
                .copied();
            months.push(ValidatorTrendMonth {
                month_key: format!("{year:04}-{month:02}"),
                month_start: local_day_bounds(
                    &query.timezone,
                    &month_first.format("%Y-%m-%d").to_string(),
                )?
                .0,
                month_end: local_day_bounds(
                    &query.timezone,
                    &next_month.format("%Y-%m-%d").to_string(),
                )?
                .0,
                observed_days: observed.range(month_first..=month_last).count() as i64,
                first_local_date: first.map(|date| date.format("%Y-%m-%d").to_string()),
                last_local_date: last.map(|date| date.format("%Y-%m-%d").to_string()),
            });
            cursor = next_month;
        }
    }
    let mut foreign_rows = 0i64;
    let mut foreign_timezones: Vec<String> = Vec::new();
    if answered_days > 0 {
        let foreign: Vec<(String, i64)> = sqlx::query_as(
            "SELECT timezone, COUNT(*) FROM validator_daily_snapshots WHERE validator_id = $1 AND timezone != $2 AND local_date >= $3 AND local_date <= $4 GROUP BY timezone ORDER BY timezone",
        )
        .bind(&query.validator_id)
        .bind(&query.timezone)
        .bind(answered_from_date.format("%Y-%m-%d").to_string())
        .bind(to_date.format("%Y-%m-%d").to_string())
        .fetch_all(db.pool())
        .await?;
        for (timezone, count) in foreign {
            foreign_rows += count;
            foreign_timezones.push(timezone);
        }
    }
    let mut associations =
        list_validator_associations(db, &query.validator_id, TREND_MAX_ASSOCIATIONS + 1).await?;
    let associations_truncated = associations.len() as i64 > TREND_MAX_ASSOCIATIONS;
    if associations_truncated {
        associations.truncate(TREND_MAX_ASSOCIATIONS as usize);
    }
    let deleted_nodes = match get_validator(db, &query.validator_id).await? {
        Some(validator) => count_deleted_nodes(db, &validator.network_key).await?,
        None => 0,
    };
    let observed_days = points.len() as i64;
    Ok(ValidatorTrendPage {
        timezone: query.timezone.clone(),
        requested_from: format_rfc3339(requested_from),
        requested_to: format_rfc3339(requested_to),
        requested_from_local_date: requested_from_date.format("%Y-%m-%d").to_string(),
        requested_to_local_date: requested_to_date.format("%Y-%m-%d").to_string(),
        answered_from_local_date: answered_from_date.format("%Y-%m-%d").to_string(),
        answered_to_local_date: to_date.format("%Y-%m-%d").to_string(),
        requested_days,
        clamped,
        expected_days: answered_days,
        observed_days,
        missing_days: answered_days - observed_days,
        gaps,
        first_observed_local_date: points.first().map(|point| point.local_date.clone()),
        last_observed_local_date: points.last().map(|point| point.local_date.clone()),
        points,
        months,
        truncated,
        continuation: if truncated { oldest_date.clone() } else { None },
        foreign_rows,
        foreign_timezones,
        associations,
        associations_truncated,
        deleted_nodes,
        association_history_partial: deleted_nodes > 0,
    })
}

/// The configured local day one detail read stored its snapshot under (#219).
/// A ranking answer is attached to this day, so a rank is never written onto a
/// day the same refresh cycle did not observe.
pub(super) struct StoredDay {
    pub(super) local_date: String,
    pub(super) month_key: String,
}

/// Store one detail reading on its configured local day, replacing the stored
/// row only when this reading is newer. Returns whether the write changed
/// stored rows together with the day it was addressed to, so the caller can
/// place same-cycle ranking evidence on exactly that day (#219).
pub(super) async fn record_daily_snapshot(
    tx: &mut Transaction<'_, Any>,
    validator_id: &str,
    source: &str,
    observation: &ValidatorObservation,
    observation_key: &str,
    received_at: &str,
    timezone: &str,
) -> Result<(bool, StoredDay), ValidatorError> {
    let (local_date, month_key, sample_at) = analytics_period(observation, received_at, timezone)?;
    let result = sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) ON CONFLICT(validator_id, timezone, local_date) DO UPDATE SET snapshot_id=excluded.snapshot_id, month_key=excluded.month_key, sample_at=excluded.sample_at, received_at=excluded.received_at, provider_timestamp=excluded.provider_timestamp, source=excluded.source, observation_key=excluded.observation_key, stake_amount=excluded.stake_amount, reward_amount=excluded.reward_amount, reward_rate=excluded.reward_rate, delegator_count=excluded.delegator_count, epoch=excluded.epoch, block_count=excluded.block_count WHERE excluded.sample_at > validator_daily_snapshots.sample_at OR (excluded.sample_at = validator_daily_snapshots.sample_at AND excluded.observation_key > validator_daily_snapshots.observation_key)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(validator_id)
        .bind(timezone)
        .bind(&local_date)
        .bind(&month_key)
        .bind(&sample_at)
        .bind(received_at)
        .bind(observation.provider_timestamp.as_deref())
        .bind(bounded_source(source))
        .bind(observation_key)
        .bind(observation.stake_amount.as_deref())
        .bind(observation.reward_amount.as_deref())
        .bind(observation.reward_rate.as_deref())
        .bind(observation.delegator_count)
        .bind(observation.epoch)
        .bind(observation.block_count)
        .execute(&mut **tx)
        .await?;
    Ok((
        result.rows_affected() > 0,
        StoredDay {
            local_date,
            month_key,
        },
    ))
}

/// Store one rank reading on a day this refresh cycle observed, and refresh
/// that month's aggregate because the aggregate caches the month's rank range
/// and last reading (#219).
///
/// The day is the one the cycle's own detail read stored, never the receipt
/// day: a rank therefore carries the calendar placement and the observation
/// time of the reading it arrived with, and a cycle that stored no snapshot
/// leaves every retained rank untouched instead of attaching a fresh rank to
/// an older day's evidence. Rows stored under another timezone are never
/// touched, and a missing row for the requested day is a no-op because a
/// ranking answer never invents a day nobody observed.
pub(super) async fn record_snapshot_rank(
    tx: &mut Transaction<'_, Any>,
    validator_id: &str,
    timezone: &str,
    rank: Option<i64>,
    day: &StoredDay,
    now: &str,
) -> Result<(), ValidatorError> {
    let stored = sqlx::query(
        "UPDATE validator_daily_snapshots SET rank = $1 WHERE validator_id = $2 AND timezone = $3 AND local_date = $4",
    )
    .bind(rank)
    .bind(validator_id)
    .bind(timezone)
    .bind(&day.local_date)
    .execute(&mut **tx)
    .await?;
    if stored.rows_affected() == 0 {
        return Ok(());
    }
    rebuild_monthly_aggregate(tx, validator_id, timezone, &day.month_key, now).await
}

pub(super) async fn rebuild_monthly_aggregate(
    tx: &mut Transaction<'_, Any>,
    validator_id: &str,
    timezone: &str,
    month_key: &str,
    updated_at: &str,
) -> Result<(), ValidatorError> {
    let summary = sqlx::query_as::<_, (i64, String, String, Option<i64>, Option<i64>)>(
        "SELECT COUNT(*), MIN(sample_at), MAX(sample_at), MIN(rank), MAX(rank) FROM validator_daily_snapshots WHERE validator_id = $1 AND timezone = $2 AND month_key = $3",
    )
    .bind(validator_id)
    .bind(timezone)
    .bind(month_key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max)) = summary
    else {
        return Ok(());
    };
    let latest = sqlx::query_as::<_, (Option<i64>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, NULL FROM validator_daily_snapshots WHERE validator_id = $1 AND timezone = $2 AND month_key = $3 ORDER BY sample_at DESC, observation_key DESC LIMIT 1",
    )
    .bind(validator_id)
    .bind(timezone)
    .bind(month_key)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO validator_monthly_aggregates (aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max, rank_last, stake_last, reward_last, reward_rate_last, delegator_count_last, epoch_last, block_count_last, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) ON CONFLICT(validator_id, timezone, month_key) DO UPDATE SET snapshot_count=excluded.snapshot_count, first_sample_at=excluded.first_sample_at, last_sample_at=excluded.last_sample_at, rank_min=excluded.rank_min, rank_max=excluded.rank_max, rank_last=excluded.rank_last, stake_last=excluded.stake_last, reward_last=excluded.reward_last, reward_rate_last=excluded.reward_rate_last, delegator_count_last=excluded.delegator_count_last, epoch_last=excluded.epoch_last, block_count_last=excluded.block_count_last, updated_at=excluded.updated_at")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(validator_id)
        .bind(timezone)
        .bind(month_key)
        .bind(snapshot_count)
        .bind(first_sample_at)
        .bind(last_sample_at)
        .bind(rank_min)
        .bind(rank_max)
        .bind(latest.0)
        .bind(latest.1)
        .bind(latest.2)
        .bind(latest.3)
        .bind(latest.4)
        .bind(latest.5)
        .bind(latest.6)
        .bind(updated_at)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use time::OffsetDateTime;

    use crate::database::ServerDatabase;

    use crate::validator::error::ValidatorError;
    use crate::validator::insight::load_insight;
    use crate::validator::provider::{
        RankingProviderResult, ValidatorObservation, ValidatorProviderResult,
    };
    use crate::validator::refresh::{RefreshSummary, refresh_all_with_channels_in_timezone};
    use crate::validator::registry::{create_link, create_validator};
    use crate::validator::test_helpers::{FakeProvider, ranking_with, test_db};

    use super::*;

    fn utc(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).expect("a valid test instant")
    }

    fn instant(value: &str) -> OffsetDateTime {
        OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
            .expect("a valid RFC3339 test instant")
    }

    fn trend_query(validator_id: &str, timezone: &str) -> ValidatorTrendQuery {
        ValidatorTrendQuery {
            validator_id: validator_id.to_owned(),
            timezone: timezone.to_owned(),
            ..ValidatorTrendQuery::default()
        }
    }

    /// One Validator on the test Node under the automatic-identity model, so a
    /// trend answer has an association it can still resolve (#173, #219).
    async fn seed_trend_validator(pool: &sqlx::AnyPool) {
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-1', 'platon-mainnet', '0x01', 'First', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ('link-1', 'node-1', 'validator-1', NULL, 'automatic', '2026-01-01T00:00:00Z', NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(pool)
            .await
            .unwrap();
    }

    /// One durable daily snapshot as the writer leaves it: the bucket is the
    /// configured local date, and sample_at is the Provider timestamp when the
    /// observation carried one and the receipt time otherwise.
    async fn seed_trend_day(
        pool: &sqlx::AnyPool,
        timezone: &str,
        local_date: &str,
        received_at: &str,
        provider_timestamp: Option<&str>,
    ) {
        sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES ($1, 'validator-1', $2, $3, $4, $5, $6, $7, 'platsScan', $8, 12, '1000.000000', '25.000000', '0.05', 40, 5, 900)")
            .bind(format!("snapshot-{timezone}-{local_date}"))
            .bind(timezone)
            .bind(local_date)
            .bind(&local_date[..7])
            .bind(provider_timestamp.unwrap_or(received_at))
            .bind(received_at)
            .bind(provider_timestamp)
            .bind(format!("observation-{timezone}-{local_date}"))
            .execute(pool)
            .await
            .unwrap();
    }

    #[test]
    fn a_configured_local_day_bounds_its_real_utc_stretch() {
        assert_eq!(
            local_day_bounds("UTC", "2026-01-01").unwrap(),
            (
                "2026-01-01T00:00:00Z".to_owned(),
                "2026-01-02T00:00:00Z".to_owned()
            )
        );
        // A zone east of UTC starts its local day on the previous UTC date.
        assert_eq!(
            local_day_bounds("Pacific/Kiritimati", "2026-02-01").unwrap(),
            (
                "2026-01-31T10:00:00Z".to_owned(),
                "2026-02-01T10:00:00Z".to_owned()
            )
        );
        // A quarter-hour offset is preserved rather than rounded to the hour.
        assert_eq!(
            local_day_bounds("Asia/Kathmandu", "2026-02-01").unwrap(),
            (
                "2026-01-31T18:15:00Z".to_owned(),
                "2026-02-01T18:15:00Z".to_owned()
            )
        );
        assert_eq!(
            local_date_at("Pacific/Kiritimati", utc(1767225600)).unwrap(),
            "2026-01-01"
        );
        assert_eq!(
            local_date_at("America/New_York", utc(1767225600)).unwrap(),
            "2025-12-31"
        );
        assert!(matches!(
            local_day_bounds("Not/AZone", "2026-01-01"),
            Err(ValidatorError::InvalidTimezone(_))
        ));
    }

    #[test]
    fn a_daylight_saving_day_is_twenty_three_or_twenty_five_hours() {
        // America/New_York 2026: spring forward on 2026-03-08, fall back on
        // 2026-11-01. Neither day is pretended to be 24 hours wide, and each
        // starts exactly where the previous local day ends.
        let (start, end) = local_day_bounds("America/New_York", "2026-03-08").unwrap();
        assert_eq!(start, "2026-03-08T05:00:00Z");
        assert_eq!(end, "2026-03-09T04:00:00Z");
        assert_eq!(instant(&end) - instant(&start), time::Duration::hours(23));
        let (start, end) = local_day_bounds("America/New_York", "2026-11-01").unwrap();
        assert_eq!(start, "2026-11-01T04:00:00Z");
        assert_eq!(end, "2026-11-02T05:00:00Z");
        assert_eq!(instant(&end) - instant(&start), time::Duration::hours(25));
        let (_, previous_end) = local_day_bounds("America/New_York", "2026-03-07").unwrap();
        assert_eq!(
            previous_end, "2026-03-08T05:00:00Z",
            "a DST day must start where the previous local day ends"
        );

        // America/Santiago removes local midnight itself on 2026-09-06: the day
        // starts at the first instant that really exists instead of silently
        // falling back onto the previous UTC date.
        let (start, end) = local_day_bounds("America/Santiago", "2026-09-06").unwrap();
        assert_eq!(
            local_date_at("America/Santiago", instant(&start)).unwrap(),
            "2026-09-06"
        );
        assert_eq!(instant(&end) - instant(&start), time::Duration::hours(23));
        let (_, previous_end) = local_day_bounds("America/Santiago", "2026-09-05").unwrap();
        assert_eq!(previous_end, start);
    }

    #[tokio::test]
    async fn a_configured_local_calendar_window_answers_its_own_days() {
        let (_dir, db) = test_db().await;
        seed_trend_validator(db.pool()).await;
        // 09:00 on 2026-02-01 in Asia/Tokyo: the same instant is still
        // 2026-01-31 in UTC, so a UTC-bucketed answer would lose this day.
        seed_trend_day(
            db.pool(),
            "Asia/Tokyo",
            "2026-02-01",
            "2026-02-01T00:00:30Z",
            Some("2026-02-01T00:00:00Z"),
        )
        .await;

        let mut query = trend_query("validator-1", "Asia/Tokyo");
        query.from = Some("2026-02-01T00:00:00Z".to_owned());
        query.to = Some("2026-02-01T00:00:00Z".to_owned());
        let page = load_daily_trend(&db, &query, utc(1770000000))
            .await
            .unwrap();

        assert_eq!(page.timezone, "Asia/Tokyo");
        assert_eq!(page.requested_from_local_date, "2026-02-01");
        assert_eq!(page.answered_from_local_date, "2026-02-01");
        assert_eq!(page.requested_days, 1);
        assert_eq!(page.expected_days, 1);
        assert_eq!(page.observed_days, 1);
        assert_eq!(page.missing_days, 0);
        assert!(page.gaps.is_empty());
        assert!(!page.truncated);
        assert_eq!(page.continuation, None);
        assert_eq!(page.points.len(), 1);
        let point = &page.points[0];
        assert_eq!(point.local_date, "2026-02-01");
        assert_eq!(point.day_start, "2026-01-31T15:00:00Z");
        assert_eq!(point.day_end, "2026-02-01T15:00:00Z");
        assert_eq!(point.sample_time, "provider");
        assert_eq!(point.delay_seconds, Some(30));
        assert!(!point.clock_suspect);
        assert_eq!(point.rank, Some(12));
        assert_eq!(point.stake_amount.as_deref(), Some("1000.000000"));
        assert_eq!(point.delegator_count, Some(40));
        // The month boundary comes from the configured calendar, not from UTC.
        assert_eq!(page.months.len(), 1);
        assert_eq!(page.months[0].month_key, "2026-02");
        assert_eq!(page.months[0].month_start, "2026-01-31T15:00:00Z");
        assert_eq!(page.months[0].month_end, "2026-02-28T15:00:00Z");
        assert_eq!(page.months[0].observed_days, 1);
        assert_eq!(
            page.months[0].first_local_date.as_deref(),
            Some("2026-02-01")
        );
        // The retained associations still resolve, and nothing is partial yet.
        assert_eq!(page.associations.len(), 1);
        assert_eq!(page.associations[0].node_id, "node-1");
        assert_eq!(page.associations[0].origin, "automatic");
        assert!(page.associations[0].valid_until.is_none());
        assert_eq!(page.associations[0].node_lifecycle, "active");
        assert!(!page.associations_truncated);
        assert_eq!(page.deleted_nodes, 0);
        assert!(!page.association_history_partial);
    }

    #[tokio::test]
    async fn a_day_without_a_snapshot_is_a_gap_and_an_unknown_delay_is_never_zero() {
        let (_dir, db) = test_db().await;
        seed_trend_validator(db.pool()).await;
        // 01-01 carries no Provider timestamp, so no delay exists to measure.
        seed_trend_day(db.pool(), "UTC", "2026-01-01", "2026-01-01T00:05:00Z", None).await;
        seed_trend_day(
            db.pool(),
            "UTC",
            "2026-01-03",
            "2026-01-03T00:00:30Z",
            Some("2026-01-03T00:00:00Z"),
        )
        .await;
        // 01-05 is stamped after its receipt: the Provider clock is ahead.
        seed_trend_day(
            db.pool(),
            "UTC",
            "2026-01-05",
            "2026-01-05T00:00:00Z",
            Some("2026-01-05T00:00:30Z"),
        )
        .await;

        let mut query = trend_query("validator-1", "UTC");
        query.from = Some("2026-01-01T00:00:00Z".to_owned());
        query.to = Some("2026-01-05T00:00:00Z".to_owned());
        let page = load_daily_trend(&db, &query, utc(1770000000))
            .await
            .unwrap();

        assert_eq!(page.expected_days, 5);
        assert_eq!(page.observed_days, 3);
        assert_eq!(page.missing_days, 2);
        assert_eq!(
            page.gaps,
            vec![
                ValidatorTrendGap {
                    from_local_date: "2026-01-02".to_owned(),
                    to_local_date: "2026-01-02".to_owned(),
                    days: 1,
                },
                ValidatorTrendGap {
                    from_local_date: "2026-01-04".to_owned(),
                    to_local_date: "2026-01-04".to_owned(),
                    days: 1,
                },
            ]
        );
        let dates: Vec<&str> = page
            .points
            .iter()
            .map(|point| point.local_date.as_str())
            .collect();
        assert_eq!(dates, vec!["2026-01-01", "2026-01-03", "2026-01-05"]);
        assert_eq!(page.points[0].sample_time, "receipt");
        assert_eq!(
            page.points[0].delay_seconds, None,
            "an unmeasurable delay is Unknown, never zero"
        );
        assert_eq!(page.points[1].delay_seconds, Some(30));
        assert_eq!(page.points[2].delay_seconds, Some(-30));
        assert!(page.points[2].clock_suspect);
        assert!(!page.points[1].clock_suspect);
        assert_eq!(
            page.first_observed_local_date.as_deref(),
            Some("2026-01-01")
        );
        assert_eq!(page.last_observed_local_date.as_deref(), Some("2026-01-05"));
    }

    #[tokio::test]
    async fn an_older_page_continues_without_a_hole_or_a_repeat() {
        let (_dir, db) = test_db().await;
        seed_trend_validator(db.pool()).await;
        for local_date in [
            "2026-01-01",
            "2026-01-02",
            "2026-01-03",
            "2026-01-04",
            "2026-01-05",
        ] {
            seed_trend_day(
                db.pool(),
                "UTC",
                local_date,
                &format!("{local_date}T00:01:00Z"),
                None,
            )
            .await;
        }

        let mut query = trend_query("validator-1", "UTC");
        query.from = Some("2026-01-01T00:00:00Z".to_owned());
        query.to = Some("2026-01-05T00:00:00Z".to_owned());
        query.limit = 2;
        let first = load_daily_trend(&db, &query, utc(1770000000))
            .await
            .unwrap();
        assert!(first.truncated);
        assert_eq!(first.continuation.as_deref(), Some("2026-01-04"));
        assert_eq!(first.answered_from_local_date, "2026-01-04");
        assert_eq!(first.answered_to_local_date, "2026-01-05");
        assert_eq!(first.requested_days, 5);
        assert_eq!(first.expected_days, 2);
        assert_eq!(first.observed_days, 2);
        assert!(
            first.gaps.is_empty(),
            "the unread days are paging, not a gap"
        );

        query.before = first.continuation.clone();
        let second = load_daily_trend(&db, &query, utc(1770000000))
            .await
            .unwrap();
        assert!(second.truncated);
        assert_eq!(second.continuation.as_deref(), Some("2026-01-02"));
        assert!(second.gaps.is_empty());

        query.before = second.continuation.clone();
        let third = load_daily_trend(&db, &query, utc(1770000000))
            .await
            .unwrap();
        assert!(!third.truncated);
        assert_eq!(third.continuation, None);
        assert_eq!(third.answered_from_local_date, "2026-01-01");

        let mut seen: Vec<String> = Vec::new();
        for page in [&first, &second, &third] {
            for point in &page.points {
                assert!(
                    !seen.contains(&point.local_date),
                    "a page must never repeat {}",
                    point.local_date
                );
                seen.push(point.local_date.clone());
            }
        }
        seen.sort();
        assert_eq!(
            seen,
            vec![
                "2026-01-01".to_owned(),
                "2026-01-02".to_owned(),
                "2026-01-03".to_owned(),
                "2026-01-04".to_owned(),
                "2026-01-05".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn a_foreign_timezone_row_is_counted_instead_of_merged() {
        let (_dir, db) = test_db().await;
        seed_trend_validator(db.pool()).await;
        seed_trend_day(
            db.pool(),
            "Asia/Tokyo",
            "2026-02-02",
            "2026-02-02T00:01:00Z",
            None,
        )
        .await;
        // A row bucketed under a zone the Server can no longer answer in must not
        // be silently re-bucketed into this answer's calendar.
        seed_trend_day(db.pool(), "UTC", "2026-02-02", "2026-02-02T00:02:00Z", None).await;

        let mut query = trend_query("validator-1", "Asia/Tokyo");
        query.from = Some("2026-02-02T00:00:00Z".to_owned());
        query.to = Some("2026-02-02T00:00:00Z".to_owned());
        let page = load_daily_trend(&db, &query, utc(1770000000))
            .await
            .unwrap();

        assert_eq!(page.observed_days, 1);
        assert_eq!(page.points.len(), 1);
        assert_eq!(page.foreign_rows, 1);
        assert_eq!(page.foreign_timezones, vec!["UTC".to_owned()]);
    }

    #[tokio::test]
    async fn a_window_wider_than_the_bound_is_narrowed_and_says_so() {
        let (_dir, db) = test_db().await;
        seed_trend_validator(db.pool()).await;
        seed_trend_day(db.pool(), "UTC", "2026-01-05", "2026-01-05T00:01:00Z", None).await;

        let mut query = trend_query("validator-1", "UTC");
        query.from = Some("2020-01-01T00:00:00Z".to_owned());
        query.to = Some("2026-01-05T00:00:00Z".to_owned());
        let page = load_daily_trend(&db, &query, utc(1770000000))
            .await
            .unwrap();

        assert!(page.clamped);
        assert!(page.requested_days > TREND_MAX_WINDOW_DAYS);
        assert_eq!(page.expected_days, TREND_MAX_WINDOW_DAYS);
        assert_eq!(page.answered_to_local_date, "2026-01-05");
        assert_eq!(page.requested_from, "2020-01-01T00:00:00Z");
        assert_eq!(page.requested_to, "2026-01-05T00:00:00Z");
        assert_eq!(page.observed_days, 1);

        let mut same = trend_query("validator-1", "UTC");
        same.from = Some("2026-01-05T00:00:00Z".to_owned());
        same.to = Some("2026-01-04T00:00:00Z".to_owned());
        assert!(matches!(
            load_daily_trend(&db, &same, utc(1770000000)).await,
            Err(ValidatorError::InvalidTrendWindow(_))
        ));

        // A cursor older than every answer is an honest empty page, not an
        // error and not a silent jump back to the newest days.
        let mut unusable = trend_query("validator-1", "UTC");
        unusable.before = Some("0001-01-01".to_owned());
        let empty = load_daily_trend(&db, &unusable, utc(1770000000))
            .await
            .unwrap();
        assert_eq!(empty.expected_days, 0);
        assert!(empty.points.is_empty());
        assert!(!empty.truncated);
        assert_eq!(empty.continuation, None);
        assert!(matches!(
            load_daily_trend(
                &db,
                &trend_query("validator-1", "Not/AZone"),
                utc(1770000000)
            )
            .await,
            Err(ValidatorError::InvalidTimezone(_))
        ));
    }

    #[tokio::test]
    async fn a_purged_node_leaves_the_retained_days_and_a_partial_association_notice() {
        let (_dir, db) = test_db().await;
        seed_trend_validator(db.pool()).await;
        seed_trend_day(db.pool(), "UTC", "2026-01-01", "2026-01-01T00:01:00Z", None).await;
        // Purge removes the Node's Link rows in the same transaction that
        // removes the Node, so the association can no longer be resolved and the
        // retained Validator history must stay exactly as it is (#219, design §15.4.1).
        sqlx::query("DELETE FROM node_validator_links WHERE node_id = 'node-1'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM nodes WHERE node_id = 'node-1'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO deleted_nodes (node_id, agent_id, network_key, display_name, deleted_by_user_id, deleted_at) VALUES ('node-1', 'agent-1', 'platon-mainnet', 'First', NULL, '2026-01-02T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();

        let mut query = trend_query("validator-1", "UTC");
        query.from = Some("2026-01-01T00:00:00Z".to_owned());
        query.to = Some("2026-01-01T00:00:00Z".to_owned());
        let page = load_daily_trend(&db, &query, utc(1770000000))
            .await
            .unwrap();

        assert_eq!(page.observed_days, 1);
        assert_eq!(page.points.len(), 1);
        assert_eq!(page.points[0].local_date, "2026-01-01");
        assert!(page.associations.is_empty());
        assert_eq!(page.deleted_nodes, 1);
        assert!(
            page.association_history_partial,
            "a purged Node's association is unavailable, not absent"
        );
    }

    #[test]
    fn analytics_period_uses_configured_iana_timezone_and_calendar_months() {
        let observation = ValidatorObservation {
            provider_timestamp: Some("2025-03-01T00:30:00Z".to_owned()),
            ..Default::default()
        };
        let received_at = "2025-03-02T01:00:00Z";
        let tokyo = analytics_period(&observation, received_at, "Asia/Tokyo").unwrap();
        assert_eq!(tokyo.0, "2025-03-01");
        assert_eq!(tokyo.1, "2025-03");
        assert_eq!(tokyo.2, "2025-03-01T00:30:00Z");
        let los_angeles =
            analytics_period(&observation, received_at, "America/Los_Angeles").unwrap();
        assert_eq!(los_angeles.0, "2025-02-28");
        assert_eq!(los_angeles.1, "2025-02");
        assert_eq!(los_angeles.2, "2025-03-01T00:30:00Z");

        // Without a provider timestamp, the Server receipt time is the honest
        // fallback and is converted in the same configured timezone.
        let no_provider_time = ValidatorObservation::default();
        let fallback =
            analytics_period(&no_provider_time, "2025-03-02T01:00:00Z", "Asia/Tokyo").unwrap();
        assert_eq!(fallback.0, "2025-03-02");
        assert_eq!(fallback.1, "2025-03");

        // Calendar-month rollover is local-time based.
        let month_end = ValidatorObservation {
            provider_timestamp: Some("2025-02-28T23:30:00Z".to_owned()),
            ..Default::default()
        };
        let tokyo_month_end = analytics_period(&month_end, received_at, "Asia/Tokyo").unwrap();
        assert_eq!(tokyo_month_end.0, "2025-03-01");
        assert_eq!(tokyo_month_end.1, "2025-03");

        // Daylight-saving transitions do not split a local calendar day.
        let before_dst = ValidatorObservation {
            provider_timestamp: Some("2025-03-09T06:59:00Z".to_owned()),
            ..Default::default()
        };
        let after_dst = ValidatorObservation {
            provider_timestamp: Some("2025-03-09T07:01:00Z".to_owned()),
            ..Default::default()
        };
        let before = analytics_period(&before_dst, received_at, "America/New_York").unwrap();
        let after = analytics_period(&after_dst, received_at, "America/New_York").unwrap();
        assert_eq!(before.0, "2025-03-09");
        assert_eq!(after.0, "2025-03-09");
        assert_eq!(before.1, after.1);

        assert!(matches!(
            analytics_period(&observation, received_at, "Not/AZone"),
            Err(ValidatorError::InvalidTimezone(_))
        ));
    }

    #[tokio::test]
    async fn analytics_snapshots_are_calendar_scoped_and_not_multiplied_by_linked_nodes() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, lifecycle, visibility, inventory_revision, first_seen_at, updated_at, rpc_endpoint) VALUES ('node-2', 'agent-1', 'platon-mainnet', 'active', 'private', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 'http://127.0.0.1:2')")
            .execute(db.pool())
            .await
            .unwrap();
        let (validator, _) =
            create_validator(&db, "platon-mainnet", "0xanalytics", None, &owner_id)
                .await
                .unwrap();
        create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "primary",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        create_link(
            &db,
            "node-2",
            &validator.validator_id,
            "standby",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();

        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-31T15:30:00Z".to_owned()),
                    stake_amount: Some("10".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-02-28T15:30:00Z".to_owned()),
                    stake_amount: Some("20".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-02-28T15:30:00Z".to_owned()),
                    stake_amount: Some("20".to_owned()),
                    ..Default::default()
                })),
            ]),
            ..FakeProvider::default()
        };

        let channels = crate::config::NotificationChannels::default();
        for _ in 0..3 {
            let summary =
                refresh_all_with_channels_in_timezone(&db, &provider, &channels, "Asia/Tokyo")
                    .await
                    .unwrap();
            assert_eq!(
                summary.attempted, 1,
                "one Validator is fetched once per refresh"
            );
        }
        assert_eq!(provider.calls.lock().unwrap().len(), 3);

        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(
            daily.len(),
            2,
            "two local calendar days, no per-Node duplication"
        );
        assert_eq!(daily[0].local_date, "2025-03-01");
        assert_eq!(daily[0].month_key, "2025-03");
        // Ranking is persisted independently from the detail-derived snapshot.
        assert_eq!(daily[0].rank, None);
        assert_eq!(daily[1].local_date, "2025-02-01");
        assert_eq!(daily[1].month_key, "2025-02");
        assert_eq!(daily[1].rank, None);

        let monthly = list_monthly_aggregates(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(monthly.len(), 2);
        assert_eq!(monthly[0].month_key, "2025-03");
        assert_eq!(monthly[0].snapshot_count, 1);
        assert_eq!(monthly[0].rank_last, None);
        assert_eq!(monthly[1].month_key, "2025-02");
        assert_eq!(monthly[1].snapshot_count, 1);
        assert_eq!(monthly[1].rank_last, None);
    }

    async fn refresh_kathmandu(
        db: &ServerDatabase,
        provider: &FakeProvider,
        channels: &crate::config::NotificationChannels,
    ) -> Result<RefreshSummary, ValidatorError> {
        refresh_all_with_channels_in_timezone(db, provider, channels, "Asia/Kathmandu").await
    }

    #[tokio::test]
    async fn ranking_answer_reaches_the_configured_local_day_snapshot() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xranked", None, &owner_id)
            .await
            .unwrap();
        // A detail reading stamped on the previous configured local day, used
        // by the last cycle below to place a rank on the day it describes.
        let delayed =
            crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::hours(24));
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("10".to_owned()),
                    delegator_count: Some(4),
                    ..Default::default()
                })),
                // A detail failure in a later cycle must neither invent a day
                // nor rewrite that day's rank from a reading no day observed.
                ValidatorProviderResult::Error("provider timeout".to_owned()),
                ValidatorProviderResult::Error("provider timeout".to_owned()),
                // A detail reading identical to the stored one: whether the
                // two cycles share a wall-clock second or not, the day keeps
                // the same values, so this test states the rank's placement and
                // never depends on the second the cycles happened to land in.
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("10".to_owned()),
                    delegator_count: Some(4),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some(delayed.clone()),
                    stake_amount: Some("13".to_owned()),
                    delegator_count: Some(4),
                    ..Default::default()
                })),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(240, &[("0xranked", 42)]),
                ranking_with(240, &[("0xranked", 40)]),
                RankingProviderResult::Error("ranking timeout".to_owned()),
                ranking_with(240, &[("0xother", 3)]),
                ranking_with(240, &[("0xranked", 7)]),
            ]),
            ..FakeProvider::default()
        };
        let channels = crate::config::NotificationChannels::default();
        refresh_kathmandu(&db, &provider, &channels).await.unwrap();

        let today = local_date_at("Asia/Kathmandu", crate::auth::now_utc()).unwrap();
        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(
            daily.len(),
            1,
            "a ranking answer never adds a day of its own"
        );
        assert_eq!(daily[0].local_date, today);
        assert_eq!(daily[0].timezone, "Asia/Kathmandu");
        assert_eq!(
            daily[0].rank,
            Some(42),
            "the stored day carries the rank its ranking endpoint reported"
        );
        assert_eq!(daily[0].stake_amount.as_deref(), Some("10"));
        assert_eq!(daily[0].delegator_count, Some(4));

        let monthly = list_monthly_aggregates(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(monthly[0].rank_min, Some(42));
        assert_eq!(monthly[0].rank_max, Some(42));
        assert_eq!(monthly[0].rank_last, Some(42));

        // A cycle whose detail call fails stores no day, so it writes no rank
        // either: the day's rank always arrives with the observation it
        // belongs to, and failing evidence keeps the last-good reading.
        refresh_kathmandu(&db, &provider, &channels).await.unwrap();
        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily.len(), 1, "a failed detail cycle invents no snapshot");
        assert_eq!(
            daily[0].rank,
            Some(42),
            "a cycle that observed no day writes no rank onto one"
        );
        assert_eq!(daily[0].stake_amount.as_deref(), Some("10"));

        // A failed ranking attempt keeps the last-good reading: an unavailable
        // rank is never stored as an absence of rank.
        refresh_kathmandu(&db, &provider, &channels).await.unwrap();
        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily[0].rank, Some(42));

        // An authoritative cohort that omits this Validator stores the absence
        // of a rank, which is what the ranking endpoint actually reported.
        refresh_kathmandu(&db, &provider, &channels).await.unwrap();
        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily[0].rank, None);
        assert_eq!(daily[0].stake_amount.as_deref(), Some("10"));
        let monthly = list_monthly_aggregates(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(monthly[0].rank_last, None);

        // A detail reading stamped on the previous configured local day stores
        // that day, and the same cycle's rank lands with it instead of on the
        // receipt day or on a day nobody observed.
        refresh_kathmandu(&db, &provider, &channels).await.unwrap();
        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily.len(), 2, "the delayed reading adds its own day");
        let yesterday = local_date_at(
            "Asia/Kathmandu",
            crate::auth::now_utc() - time::Duration::hours(24),
        )
        .unwrap();
        assert_eq!(daily[1].local_date, yesterday);
        assert_eq!(
            daily[1].provider_timestamp.as_deref(),
            Some(delayed.as_str())
        );
        assert_eq!(
            daily[1].rank,
            Some(7),
            "the rank is placed on the day its own cycle stored"
        );
        assert_eq!(
            daily[0].rank, None,
            "the receipt day keeps the reading it was observed with"
        );
        assert_eq!(
            daily[0].stake_amount.as_deref(),
            Some("10"),
            "the receipt day keeps the reading it was observed with"
        );
    }

    #[tokio::test]
    async fn delayed_observation_uses_provider_calendar_day_and_replay_is_idempotent() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xdelayed", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:30:00Z".to_owned()),
                    stake_amount: Some("10".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:30:00Z".to_owned()),
                    stake_amount: Some("10".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:30:00Z".to_owned()),
                    stake_amount: Some("10".to_owned()),
                    ..Default::default()
                })),
            ]),
            ..FakeProvider::default()
        };
        let channels = crate::config::NotificationChannels::default();
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "America/Los_Angeles")
            .await
            .unwrap();
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "America/Los_Angeles")
            .await
            .unwrap();

        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].local_date, "2024-12-31");
        assert_eq!(daily[0].month_key, "2024-12");
        assert_eq!(daily[0].rank, None);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = $1"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );

        // A Server restart must not replay or duplicate the accepted sample.
        db.close().await;
        let db = crate::test_support::test_database_in(_dir.path()).await;
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "America/Los_Angeles")
            .await
            .unwrap();
        let daily_after_restart = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily_after_restart.len(), 1);
        let monthly_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = $1",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(monthly_count, 1);
    }

    #[tokio::test]
    async fn provider_failure_keeps_analytics_and_marks_current_state_not_healthy() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xstate", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    stake_amount: Some("300".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Error("provider timeout".to_owned()),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xstate", 3)]),
                RankingProviderResult::Error("ranking timeout".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        let channels = crate::config::NotificationChannels::default();
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "UTC")
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM validator_daily_snapshots")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "UTC")
            .await
            .unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.rank, Some(3));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM validator_daily_snapshots")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1,
            "a failed provider attempt must not add an analytics sample"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = $1"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }
}
