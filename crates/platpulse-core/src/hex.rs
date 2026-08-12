//! Hex-string wire identifiers used by the protocol.
//!
//! Encoding rule: chain hashes and addresses are lowercase hex with a `0x`
//! prefix and a fixed length (64 nibbles for 32-byte hashes, 40 nibbles for
//! 20-byte values). Uppercase input is rejected; the canonical lowercase
//! form is the only valid wire representation.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Failure to parse a fixed-length `0x`-prefixed lowercase hex value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexError {
    /// The value does not start with `0x`.
    MissingPrefix,
    /// The value has the wrong length for its type.
    InvalidLength { expected_nibbles: usize },
    /// A character is not a lowercase hex digit (a-f, 0-9).
    NotLowercaseHex,
}

impl fmt::Display for HexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrefix => write!(f, "hex value must start with 0x"),
            Self::InvalidLength { expected_nibbles } => {
                write!(f, "hex value must have {expected_nibbles} nibbles after 0x")
            }
            Self::NotLowercaseHex => {
                write!(f, "hex value must use lowercase digits only")
            }
        }
    }
}

impl std::error::Error for HexError {}

fn parse_hex(s: &str, expected_nibbles: usize) -> Result<String, HexError> {
    let digits = s.strip_prefix("0x").ok_or(HexError::MissingPrefix)?;
    if digits.len() != expected_nibbles {
        return Err(HexError::InvalidLength { expected_nibbles });
    }
    if !digits
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(HexError::NotLowercaseHex);
    }
    Ok(s.to_owned())
}

macro_rules! hex_type {
    ($(#[$meta:meta])* $name:ident, $nibbles:expr) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, Debug)]
        pub struct $name(String);

        impl $name {
            /// The canonical `0x`-prefixed lowercase hex string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = HexError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                parse_hex(s, $nibbles).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

hex_type!(
    /// A 32-byte chain hash (`0x` + 64 lowercase hex digits).
    Hash32,
    64
);
hex_type!(
    /// SHA-256 of a report body (`0x` + 64 lowercase hex digits).
    Sha256Hex,
    64
);
hex_type!(
    /// A 20-byte address (`0x` + 40 lowercase hex digits).
    Address,
    40
);
hex_type!(
    /// A 20-byte key fingerprint (`0x` + 40 lowercase hex digits).
    FingerprintHex,
    40
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_values_parse_and_display() {
        let h: Hash32 = format!("0x{}", "a".repeat(64)).parse().unwrap();
        assert_eq!(h.as_str().len(), 66);
        assert_eq!(h.to_string(), format!("0x{}", "a".repeat(64)));
        let a: Address = format!("0x{}", "b".repeat(40)).parse().unwrap();
        assert_eq!(a.as_str().len(), 42);
    }

    #[test]
    fn rejects_bad_values() {
        assert_eq!("abc".parse::<Hash32>(), Err(HexError::MissingPrefix));
        assert_eq!(
            format!("0x{}", "a".repeat(63)).parse::<Hash32>(),
            Err(HexError::InvalidLength {
                expected_nibbles: 64
            })
        );
        assert_eq!(
            format!("0x{}", "A".repeat(64)).parse::<Hash32>(),
            Err(HexError::NotLowercaseHex)
        );
        assert_eq!(
            format!("0x{}", "g".repeat(64)).parse::<Hash32>(),
            Err(HexError::NotLowercaseHex)
        );
    }

    #[test]
    fn types_are_not_interchangeable() {
        // Same shape, distinct nominal types: a block hash is not a body hash.
        let _: Hash32 = format!("0x{}", "a".repeat(64)).parse().unwrap();
        let _: Sha256Hex = format!("0x{}", "a".repeat(64)).parse().unwrap();
        let _: Address = format!("0x{}", "a".repeat(40)).parse().unwrap();
        let _: FingerprintHex = format!("0x{}", "a".repeat(40)).parse().unwrap();
    }
}
