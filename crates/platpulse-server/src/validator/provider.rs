//! Validator Provider contract: canonical observations, ranking lists and the Provider trait.

use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const MAX_PROVIDER_DIAGNOSTIC_LEN: usize = 256;
pub const MAX_PROVIDER_BODY_LEN: usize = 64 * 1024;

/// Canonical Validator Activity values. Provider-specific statuses (including
/// numeric PlatScan statuses) never cross this boundary; adapters map their
/// upstream vocabulary into these exact values (#100, #101).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidatorActivity {
    Candidate,
    Active,
    Producing,
    Exiting,
    Exited,
    Verifying,
    Locked,
}

impl ValidatorActivity {
    pub fn as_str(&self) -> &'static str {
        match self {
            ValidatorActivity::Candidate => "candidate",
            ValidatorActivity::Active => "active",
            ValidatorActivity::Producing => "producing",
            ValidatorActivity::Exiting => "exiting",
            ValidatorActivity::Exited => "exited",
            ValidatorActivity::Verifying => "verifying",
            ValidatorActivity::Locked => "locked",
        }
    }

    pub fn from_canonical(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "candidate" => Some(ValidatorActivity::Candidate),
            "active" => Some(ValidatorActivity::Active),
            "producing" => Some(ValidatorActivity::Producing),
            "exiting" => Some(ValidatorActivity::Exiting),
            "exited" => Some(ValidatorActivity::Exited),
            "verifying" => Some(ValidatorActivity::Verifying),
            "locked" => Some(ValidatorActivity::Locked),
            _ => None,
        }
    }
}

/// Exact normalized values returned by a Server-side Validator Provider.
/// Provider-specific JSON never crosses this boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidatorObservation {
    pub provider_timestamp: Option<String>,
    pub activity: Option<ValidatorActivity>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    /// Currently effective delegation reward distribution percentage
    /// (PlatScan detail `rewardPer`) in percentage points: the source's
    /// already-scaled `20` is 20%, never 0.20% or 2000%. It is distinct from
    /// the annualized `reward_rate` and from the pending `nextRewardPer`,
    /// which is never read (#157).
    pub delegation_reward_percentage: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
    /// Cumulative scheduled blocks from the same successful observation as
    /// `block_count`. `None` means the source omitted it; `Some(0)` is an
    /// authoritative "no scheduled duties" denominator (#156).
    pub expected_block_count: Option<i64>,
    /// PlatScan's own 24-hour production rate as a percentage string without
    /// the `%` sign. It is source-reported, not a locally reconstructed
    /// rolling window (#156).
    pub gen_blocks_rate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatorProviderResult {
    /// Boxed so the result enum stays small: a normalized observation is a wide
    /// value struct that is moved once per Provider fetch.
    Success(Box<ValidatorObservation>),
    NotFound,
    AuthoritativeEmpty,
    /// No PlatScan deployment is bound to this Network. This is distinct from
    /// a deployment that answered but does not support the request: an
    /// unconfigured Network must never look like a failed or dead source
    /// (#154).
    NotConfigured(String),
    Error(String),
    Unsupported(String),
}

/// Bounded page size for the dedicated Network ranking list. The upstream
/// endpoint accepts up to 1000 rows per page; a smaller page keeps each
/// bounded response comfortably under the 64 KiB body limit.
pub const RANKING_PAGE_SIZE: usize = 50;
/// Hard bound on ranking pages fetched in one refresh.
pub const MAX_RANKING_PAGES: usize = 50;
/// Hard bound on a complete live-staking cohort accepted as authoritative.
pub const MAX_RANKING_COHORT: i64 = 2500;

/// A complete, validated Network ranking list. Ranks are the upstream
/// positions (1-based) adopted verbatim; PlatPulse never recomputes them over
/// the monitored Node set or Home filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkRanking {
    /// Ranked entries keyed by PlatScan Validator node identifier.
    pub entries: BTreeMap<String, i64>,
    /// Complete cohort size reported by the upstream list (`totalCount`).
    pub cohort_size: i64,
}

/// The outcome of one dedicated Network ranking fetch. Ranking is independent
/// of the per-Validator detail request: a detail failure never triggers a
/// ranking fallback, and a ranking failure never erases detail metrics (#158).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RankingProviderResult {
    /// A complete, internally consistent list. Only this establishes an
    /// authoritative unranked outcome for an absent Validator.
    Success(Box<NetworkRanking>),
    NotConfigured(String),
    Unsupported(String),
    /// A request failure or a truncated, duplicated, or drifted pagination
    /// sequence. It never clears a last-good rank.
    Error(String),
}

#[async_trait]
pub trait ValidatorProvider: Send + Sync {
    fn source(&self) -> &str;

    async fn fetch(&self, network_key: &str, validator_node_id: &str) -> ValidatorProviderResult;

    /// Fetch the Network's complete live-staking ranking list. Implementations
    /// that only serve per-Validator detail return `Unsupported` unless they
    /// override this method (#158).
    async fn fetch_ranking(&self, _network_key: &str) -> RankingProviderResult {
        RankingProviderResult::Unsupported("provider does not support Network ranking".to_owned())
    }
}

pub type SharedValidatorProvider = Arc<dyn ValidatorProvider>;

/// The default provider is explicit rather than pretending that no provider
/// means a zero-valued Validator.
#[derive(Debug, Default)]
pub struct DisabledValidatorProvider;

#[async_trait]
impl ValidatorProvider for DisabledValidatorProvider {
    fn source(&self) -> &str {
        "disabled"
    }

    async fn fetch(&self, _network_key: &str, _validator_node_id: &str) -> ValidatorProviderResult {
        ValidatorProviderResult::NotConfigured("provider is not configured".to_owned())
    }

    async fn fetch_ranking(&self, _network_key: &str) -> RankingProviderResult {
        RankingProviderResult::NotConfigured("provider is not configured".to_owned())
    }
}

pub(super) fn provider_diagnostic(value: String) -> String {
    let value = crate::redaction::redact_sensitive(&value)
        .replace("https://", "[redacted-url]/")
        .replace("http://", "[redacted-url]/");
    value.chars().take(MAX_PROVIDER_DIAGNOSTIC_LEN).collect()
}
