//! Error type shared by the Validator model.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidatorError {
    #[error("validator Node ID must be 1..=256 characters without control characters")]
    InvalidValidatorNodeId,
    #[error(
        "validator display name must be empty or 1..=128 characters without control characters"
    )]
    InvalidDisplayName,
    #[error("link role must be primary, standby, or observer")]
    InvalidRole,
    #[error("invalid RFC3339 timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("valid_until must be later than valid_from")]
    InvalidValidity,
    #[error("network was not found")]
    NetworkNotFound,
    #[error("validator identity is already registered for this Network")]
    ValidatorAlreadyExists,
    #[error("validator was not found")]
    ValidatorNotFound,
    #[error("Node was not found")]
    NodeNotFound,
    #[error("Node is not active")]
    NodeNotActive,
    #[error("validator and Node belong to different Networks")]
    NetworkMismatch,
    #[error("Node already has an overlapping Validator Link")]
    LinkOverlap,
    #[error("Validator Link was not found")]
    LinkNotFound,
    #[error("cannot end a link before its valid-from boundary")]
    EndBeforeStart,
    #[error("cannot update a link that has already ended")]
    LinkAlreadyEnded,
    #[error("a link replacement must begin after the existing link")]
    LinkReplacementMustAdvance,
    #[error("invalid Validator analytics IANA timezone: {0}")]
    InvalidTimezone(String),
    #[error("invalid Validator trend window: {0}")]
    InvalidTrendWindow(String),
    #[error("provider returned an invalid Validator observation: {0}")]
    InvalidProviderObservation(String),
    #[error("alert evaluation failed: {0}")]
    Alert(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}
