//! RFC 3339 UTC timestamp wire type.
//!
//! Encoding rule: every wall-clock timestamp on the Agent→Server wire is
//! RFC 3339 in UTC. The canonical serialized form is
//! `YYYY-MM-DDTHH:MM:SSZ` (seconds precision, `Z` suffix). Deserialization
//! accepts optional fractional seconds and the `+00:00` offset and
//! normalizes both at parse time: the stored value is always the canonical
//! seconds-precision form, so serialize→deserialize is stable and `Ord`
//! never depends on dropped sub-second digits. Any non-UTC offset is a
//! contract violation.
//!
//! Sub-second wall-clock precision is deliberately not preserved on the
//! wire; block-level precision travels in the dedicated `*_ms` integer
//! fields (e.g. `block_timestamp_ms`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::format_description::FormatItem;
use time::format_description::well_known::Rfc3339 as Rfc3339Format;
use time::{OffsetDateTime, UtcOffset};

/// Canonical seconds-precision UTC formatter: `YYYY-MM-DDTHH:MM:SSZ`.
const CANONICAL_FORMAT: &[FormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

/// A validated RFC 3339 UTC timestamp with canonical wire representation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Rfc3339(OffsetDateTime);

impl Rfc3339 {
    /// The wrapped [`OffsetDateTime`], always at UTC offset.
    pub fn as_datetime(self) -> OffsetDateTime {
        self.0
    }
}

impl fmt::Display for Rfc3339 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .format(&CANONICAL_FORMAT)
            .map_err(|_| fmt::Error)
            .and_then(|s| f.write_str(&s))
    }
}

impl FromStr for Rfc3339 {
    type Err = Rfc3339ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed =
            OffsetDateTime::parse(s, &Rfc3339Format).map_err(Rfc3339ParseError::Invalid)?;
        if parsed.offset() != UtcOffset::UTC {
            return Err(Rfc3339ParseError::NotUtc);
        }
        // Truncate sub-second digits at parse time so the stored value is
        // exactly the canonical wire form (seconds precision, UTC).
        let truncated = parsed
            .replace_nanosecond(0)
            .expect("nanosecond 0 is always in range");
        Ok(Self(truncated))
    }
}

impl Serialize for Rfc3339 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Rfc3339 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Failure to parse a wire timestamp as canonical RFC 3339 UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rfc3339ParseError {
    /// The string is not a valid RFC 3339 timestamp.
    Invalid(time::error::Parse),
    /// The timestamp is valid RFC 3339 but uses a non-UTC offset.
    NotUtc,
}

impl fmt::Display for Rfc3339ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(inner) => write!(f, "invalid RFC 3339 timestamp: {inner}"),
            Self::NotUtc => {
                write!(f, "timestamp must be UTC (Z or +00:00 offset)")
            }
        }
    }
}

impl std::error::Error for Rfc3339ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_round_trips() {
        let t: Rfc3339 = "2026-08-12T10:00:00Z".parse().unwrap();
        assert_eq!(t.to_string(), "2026-08-12T10:00:00Z");
        assert_eq!(t.as_datetime().unix_timestamp(), 1_786_528_800);
    }

    #[test]
    fn zero_utc_offset_is_normalized_to_z() {
        let t: Rfc3339 = "2026-08-12T10:00:00+00:00".parse().unwrap();
        assert_eq!(t.to_string(), "2026-08-12T10:00:00Z");
    }

    #[test]
    fn fractional_seconds_are_truncated_to_canonical() {
        let t: Rfc3339 = "2026-08-12T10:00:00.123Z".parse().unwrap();
        assert_eq!(t.to_string(), "2026-08-12T10:00:00Z");
        // Truncation happens at parse time: the stored value IS canonical,
        // so serialize→deserialize never changes equality or ordering.
        assert_eq!(t, "2026-08-12T10:00:00Z".parse().unwrap());
        assert_eq!(t, "2026-08-12T10:00:00.999Z".parse().unwrap());
        let t_plus: Rfc3339 = "2026-08-12T10:00:00.001Z".parse().unwrap();
        let t_next: Rfc3339 = "2026-08-12T10:00:01Z".parse().unwrap();
        assert!(t_plus < t_next);
    }

    #[test]
    fn non_utc_offset_is_rejected() {
        assert_eq!(
            "2026-08-12T10:00:00+02:00".parse::<Rfc3339>(),
            Err(Rfc3339ParseError::NotUtc)
        );
    }

    #[test]
    fn malformed_input_is_rejected() {
        for bad in [
            "2026-08-12 10:00:00Z",
            "2026-08-12T10:00:00",
            "2026-08-12T10:00:00ZZ",
            "not-a-timestamp",
            "2026-13-12T10:00:00Z",
        ] {
            assert!(bad.parse::<Rfc3339>().is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn ordering_matches_wall_clock() {
        let a: Rfc3339 = "2026-08-12T10:00:00Z".parse().unwrap();
        let b: Rfc3339 = "2026-08-12T10:00:01Z".parse().unwrap();
        assert!(a < b);
    }
}
