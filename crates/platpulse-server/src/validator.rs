//! Server-managed Validator identities and explicit Node Validator Links.
//!
//! Validators are keyed by `(Network, validator_node_id)` and are never
//! inferred from Agent reports, consensus membership, provider data, or
//! Node identity. Link mutations are transactional with their Audit Event and
//! reject every temporal overlap for one Node before inserting or updating.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use chrono::{DateTime, Datelike, TimeZone, Utc};
use chrono_tz::Tz;
use reqwest::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Sqlite, Transaction};
use std::sync::Arc;
use thiserror::Error;
use time::OffsetDateTime;

use crate::auth::{format_rfc3339, insert_audit_event, now_utc};
use crate::database::ServerDatabase;

pub const MAX_VALIDATOR_NODE_ID_LEN: usize = 256;
pub const MAX_VALIDATOR_DISPLAY_NAME_LEN: usize = 128;
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

pub const MAX_PROVIDER_NETWORKS: usize = 64;
pub const MAX_PROVIDER_NETWORK_KEY_LEN: usize = 128;

/// Server-side PlatScan adapter for the per-Validator `stakingDetails`
/// endpoint. Its response is deliberately reduced to the normalized
/// observation above; unknown fields and response diagnostics are discarded
/// at the trust boundary (#101).
///
/// Each Network is bound to its own deployment base URL. The detail request
/// identifies a Validator but carries no Network selector, so a shared
/// allowlist cannot establish which chain a deployment serves; an unbound
/// Network is `NotConfigured` and never queried (#154).
#[derive(Clone)]
pub struct PlatScanValidatorProvider {
    client: reqwest::Client,
    deployments: BTreeMap<String, String>,
}

impl PlatScanValidatorProvider {
    pub fn new(
        deployments: BTreeMap<String, String>,
        timeout: std::time::Duration,
    ) -> Result<Self, String> {
        validate_provider_deployments(&deployments)?;
        let mut normalized = BTreeMap::new();
        for (network_key, base_url) in &deployments {
            normalized.insert(network_key.clone(), normalize_provider_base_url(base_url)?);
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| "unable to construct PlatScan client".to_owned())?;
        Ok(Self {
            client,
            deployments: normalized,
        })
    }

    fn endpoint(base_url: &str) -> String {
        format!("{base_url}/browser-server/staking/stakingDetails")
    }

    fn deployment(&self, network_key: &str) -> Option<&str> {
        self.deployments.get(network_key).map(String::as_str)
    }

    fn ranking_endpoint(base_url: &str) -> String {
        format!("{base_url}/browser-server/staking/aliveStakingList")
    }
}

#[async_trait]
impl ValidatorProvider for PlatScanValidatorProvider {
    fn source(&self) -> &str {
        "platscan"
    }

    async fn fetch(&self, network_key: &str, validator_node_id: &str) -> ValidatorProviderResult {
        let Some(base_url) = self.deployment(network_key) else {
            return ValidatorProviderResult::NotConfigured(
                "Network has no bound PlatScan deployment".to_owned(),
            );
        };
        if !is_platscan_node_id(validator_node_id) {
            return ValidatorProviderResult::Unsupported(
                "Validator node identifier is not supported by PlatScan".to_owned(),
            );
        }
        let body = serde_json::json!({ "nodeId": validator_node_id });
        let response = match self
            .client
            .post(Self::endpoint(base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => return ValidatorProviderResult::Error("PlatScan request failed".to_owned()),
        };
        match response.status() {
            // An absent staking identity is a 200 empty form, so a 404 can only
            // be a routing, gateway, or deployment anomaly. It is a degraded
            // non-negative outcome, never proof of absence (#168).
            StatusCode::NOT_FOUND => {
                return ValidatorProviderResult::Error(
                    "PlatScan stakingDetails endpoint was not found".to_owned(),
                );
            }
            StatusCode::NOT_IMPLEMENTED | StatusCode::METHOD_NOT_ALLOWED => {
                return ValidatorProviderResult::Unsupported(
                    "PlatScan stakingDetails endpoint is unsupported".to_owned(),
                );
            }
            status if status.is_client_error() || status.is_server_error() => {
                return ValidatorProviderResult::Error(
                    "PlatScan returned an unsuccessful response".to_owned(),
                );
            }
            _ => {}
        }
        let body = match response.bytes().await {
            Ok(body) if body.len() <= MAX_PROVIDER_BODY_LEN => body,
            Ok(_) => {
                return ValidatorProviderResult::Error(
                    "PlatScan response exceeded the size limit".to_owned(),
                );
            }
            Err(_) => {
                return ValidatorProviderResult::Error(
                    "PlatScan response could not be read".to_owned(),
                );
            }
        };
        let value: Value = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(_) => {
                return ValidatorProviderResult::Error(
                    "PlatScan response was malformed".to_owned(),
                );
            }
        };
        match normalize_platscan_response(&value, validator_node_id) {
            Ok(Some(observation)) => ValidatorProviderResult::Success(Box::new(observation)),
            Ok(None) => ValidatorProviderResult::AuthoritativeEmpty,
            Err(error) => ValidatorProviderResult::Error(error),
        }
    }

    async fn fetch_ranking(&self, network_key: &str) -> RankingProviderResult {
        let Some(base_url) = self.deployment(network_key) else {
            return RankingProviderResult::NotConfigured(
                "Network has no bound PlatScan deployment".to_owned(),
            );
        };
        let mut entries = BTreeMap::new();
        let mut expected_total: Option<i64> = None;
        let mut page_no = 1usize;
        loop {
            if page_no > MAX_RANKING_PAGES {
                return RankingProviderResult::Error(
                    "PlatScan ranking exceeded the page bound".to_owned(),
                );
            }
            let body = serde_json::json!({
                "pageNo": page_no,
                "pageSize": RANKING_PAGE_SIZE,
                "queryStatus": "all",
            });
            let response = match self
                .client
                .post(Self::ranking_endpoint(base_url))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(response) => response,
                Err(_) => {
                    return RankingProviderResult::Error(
                        "PlatScan ranking request failed".to_owned(),
                    );
                }
            };
            match response.status() {
                StatusCode::NOT_FOUND
                | StatusCode::NOT_IMPLEMENTED
                | StatusCode::METHOD_NOT_ALLOWED => {
                    return RankingProviderResult::Unsupported(
                        "PlatScan aliveStakingList endpoint is unsupported".to_owned(),
                    );
                }
                status if status.is_client_error() || status.is_server_error() => {
                    return RankingProviderResult::Error(
                        "PlatScan returned an unsuccessful ranking response".to_owned(),
                    );
                }
                _ => {}
            }
            let bytes = match response.bytes().await {
                Ok(bytes) if bytes.len() <= MAX_PROVIDER_BODY_LEN => bytes,
                Ok(_) => {
                    return RankingProviderResult::Error(
                        "PlatScan ranking response exceeded the size limit".to_owned(),
                    );
                }
                Err(_) => {
                    return RankingProviderResult::Error(
                        "PlatScan ranking response could not be read".to_owned(),
                    );
                }
            };
            let value: Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => {
                    return RankingProviderResult::Error(
                        "PlatScan ranking response was malformed".to_owned(),
                    );
                }
            };
            let page = match normalize_platscan_ranking_response(&value, page_no, RANKING_PAGE_SIZE)
            {
                Ok(page) => page,
                Err(error) => return RankingProviderResult::Error(error),
            };
            match expected_total {
                Some(total) if total != page.total_count => {
                    return RankingProviderResult::Error(
                        "PlatScan ranking page count drifted".to_owned(),
                    );
                }
                None => {
                    if page.total_count > MAX_RANKING_COHORT {
                        return RankingProviderResult::Error(
                            "PlatScan ranking cohort exceeded the bound".to_owned(),
                        );
                    }
                    expected_total = Some(page.total_count);
                }
                _ => {}
            }
            let rows = page.entries.len();
            for (node_id, rank) in page.entries {
                if entries.insert(node_id, rank).is_some() {
                    return RankingProviderResult::Error(
                        "PlatScan ranking contained a duplicate Validator".to_owned(),
                    );
                }
            }
            let total = expected_total.unwrap_or_default();
            let collected = entries.len() as i64;
            if collected == total {
                break;
            }
            if collected > total || rows < RANKING_PAGE_SIZE {
                return RankingProviderResult::Error(
                    "PlatScan ranking response was incomplete".to_owned(),
                );
            }
            page_no += 1;
        }
        RankingProviderResult::Success(Box::new(NetworkRanking {
            entries,
            cohort_size: expected_total.unwrap_or_default(),
        }))
    }
}

/// Normalize an absolute HTTP(S) Provider base URL. Paths are allowed (the
/// deployment may serve PlatScan under a prefix), but credentials, query
/// strings, fragments, invalid hosts, and malformed URLs are rejected.
pub fn normalize_provider_base_url(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    let Some(rest) = value.split_once("://").map(|(_, rest)| rest) else {
        return Err("PlatScan base URL must be an absolute HTTP(S) URL".to_owned());
    };
    if rest.is_empty() || rest.starts_with('/') {
        return Err("PlatScan base URL must be an absolute HTTP(S) URL with a host".to_owned());
    }
    let parsed = url::Url::parse(value)
        .map_err(|_| "PlatScan base URL must be an absolute HTTP(S) URL".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "PlatScan base URL must be an absolute HTTP(S) URL without credentials, query strings, or fragments"
                .to_owned(),
        );
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

/// Explicit Network to deployment bindings are bounded and must be explicit:
/// an unbound Network is NotConfigured and never reaches PlatScan. A
/// BTreeMap cannot contain duplicate keys, so uniqueness is structural.
pub fn validate_provider_deployments(deployments: &BTreeMap<String, String>) -> Result<(), String> {
    if deployments.is_empty() {
        return Err(
            "PlatScan deployments must bind at least one registered Network key".to_owned(),
        );
    }
    if deployments.len() > MAX_PROVIDER_NETWORKS {
        return Err(format!(
            "PlatScan deployments are limited to {MAX_PROVIDER_NETWORKS} Network keys"
        ));
    }
    for (network, base_url) in deployments {
        if network.is_empty()
            || network.trim() != network
            || network.chars().count() > MAX_PROVIDER_NETWORK_KEY_LEN
            || network.chars().any(char::is_control)
        {
            return Err("PlatScan deployments contain an invalid Network key".to_owned());
        }
        if base_url.trim().is_empty() {
            return Err(format!(
                "PlatScan deployment for Network {network} has an empty base URL"
            ));
        }
    }
    Ok(())
}

/// PlatScan validator addresses are `0x` followed by exactly 128 hexadecimal
/// characters. Anything else is unsupported and never leaves the Server.
pub fn is_platscan_node_id(value: &str) -> bool {
    value.len() == 130
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Extract the full 64-byte P2P public key from an observed enode URI as the
/// canonical `0x` + 128 lowercase hex lookup identifier (#173).
///
/// The P2P public key is the chain identity, not a PlatPulse Node UUID, a
/// shortened fingerprint, a display name, or an IP address. A malformed or
/// shortened key is never truncated into a lookup candidate: it stays an
/// explicit unidentified reason.
pub fn observed_p2p_public_key(enode: &str) -> Result<String, String> {
    let rest = enode
        .strip_prefix("enode://")
        .ok_or_else(|| "observed enode must start with enode://".to_owned())?;
    let key = rest.split('@').next().unwrap_or_default();
    let digits = key.strip_prefix("0x").unwrap_or(key);
    if digits.len() != 128 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("observed enode public key must be 64 bytes of hexadecimal".to_owned());
    }
    Ok(format!("0x{}", digits.to_ascii_lowercase()))
}

/// Project the canonical last-good Activity and its currency for a projection
/// surface. Provider outcomes never fabricate a value: authoritative absence is
/// Observing, a successful snapshot shows the canonical label (Stale when
/// Server freshness expired), and Error with a last-good Activity is always
/// Stale. Unsupported coverage projects Unknown even when a last-good Activity
/// was previously observed (#100, #101, #168).
pub fn project_activity(
    outcome: &str,
    activity: Option<&str>,
    freshness: &str,
) -> (String, String) {
    match outcome {
        "empty" => (
            "observing".to_owned(),
            match freshness {
                "fresh" => "current",
                "stale" => "stale",
                _ => "unknown",
            }
            .to_owned(),
        ),
        // The deployment answers an absent staking identity with a 200 empty
        // object; a 404 can only come from routing or a deployment anomaly, so
        // it is never presented as an observing Validator (#168).
        "not_found" => ("unknown".to_owned(), "unknown".to_owned()),
        "success" => match activity {
            Some(value) => (
                value.to_owned(),
                match freshness {
                    "fresh" => "current",
                    "stale" => "stale",
                    _ => "unknown",
                }
                .to_owned(),
            ),
            None => ("unknown".to_owned(), "unknown".to_owned()),
        },
        "error" => match activity {
            Some(value) => (value.to_owned(), "stale".to_owned()),
            None => ("unknown".to_owned(), "unknown".to_owned()),
        },
        "unsupported" | "not_configured" => ("unknown".to_owned(), "unknown".to_owned()),
        _ => ("unknown".to_owned(), "unknown".to_owned()),
    }
}

/// Sanitized, non-sensitive explanation for a Node whose automatic Validator
/// identity is not established (#173, #218). `identified` has no reason, and a
/// caller that does not project the discovery dimension at all passes `None`.
/// Every other state is explained without disclosing secrets and without
/// implying ownership or consensus membership.
pub fn automatic_identity_reason(state: Option<&str>) -> Option<String> {
    match state {
        Some("identified") | None => None,
        Some("not_evaluated") => Some(
            "No automatic Validator identity evaluation has been recorded for this Node yet."
                .to_owned(),
        ),
        Some("missing_public_key") => Some(
            "No full P2P public key has been observed for this Node, so no Validator can be identified."
                .to_owned(),
        ),
        Some("invalid_public_key") => Some(
            "The observed P2P public key could not be validated, so no Validator was searched."
                .to_owned(),
        ),
        Some("network_identity_missing") => Some(
            "No Network Identity has been observed for this Node, so no Validator can be identified."
                .to_owned(),
        ),
        Some("network_identity_mismatch") => Some(
            "The observed Network Identity does not match this Node's registered Network; no cross-Network Validator was searched."
                .to_owned(),
        ),
        Some(_) => Some("No Validator identity has been established for this Node.".to_owned()),
    }
}

/// Current Validator Status of an automatically identified Node identity: the
/// currently valid staking identity, not current consensus selection or Node
/// Health (#173, main design §15.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentValidatorStatus {
    Validator,
    NotValidator,
    Unknown,
}

impl CurrentValidatorStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CurrentValidatorStatus::Validator => "validator",
            CurrentValidatorStatus::NotValidator => "not_validator",
            CurrentValidatorStatus::Unknown => "unknown",
        }
    }
}

/// The special state shown while a staking identity is confirmed valid but is
/// not normally participating: a low-production lock, or a withdrawal that has
/// not completed. Neither is a negative verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentValidatorQualifier {
    Locked,
    Exiting,
}

impl CurrentValidatorQualifier {
    pub fn as_str(&self) -> &'static str {
        match self {
            CurrentValidatorQualifier::Locked => "locked",
            CurrentValidatorQualifier::Exiting => "exiting",
        }
    }
}

/// Server-owned Current Validator Status plus its currency. `state` is
/// `current`, `stale`, or `unknown`; a retained last-good value after a
/// failed refresh is never presented as fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentValidatorStatusView {
    pub status: CurrentValidatorStatus,
    pub state: &'static str,
    pub qualifier: Option<CurrentValidatorQualifier>,
}

fn classify_activity(
    activity: Option<&str>,
) -> (CurrentValidatorStatus, Option<CurrentValidatorQualifier>) {
    match activity {
        Some("candidate") | Some("active") | Some("producing") => {
            (CurrentValidatorStatus::Validator, None)
        }
        Some("exiting") => (
            CurrentValidatorStatus::Validator,
            Some(CurrentValidatorQualifier::Exiting),
        ),
        Some("locked") => (
            CurrentValidatorStatus::Validator,
            Some(CurrentValidatorQualifier::Locked),
        ),
        Some("exited") | Some("observing") => (CurrentValidatorStatus::NotValidator, None),
        // The investigated source maps 6 to "candidate in a consensus round"
        // (getCodeByStatus(CANDIDATE, isConsensus = 1, *)): the stake is in
        // force and only the consensus-round dimension differs. Reading it as
        // Unknown made a Node taking part in the current round look less
        // confirmed than an idle candidate and made the Home Validator bucket
        // flap, so it classifies as Validator (Owner decision 2026-10-08,
        // amending CONTEXT.md and ADR 0005; see #168 evidence note §4.2).
        Some("verifying") => (CurrentValidatorStatus::Validator, None),
        _ => (CurrentValidatorStatus::Unknown, None),
    }
}

/// Map the canonical last-good outcome and Activity to Current Validator
/// Status using only the evidence predicates established in #168. HTTP 404 is
/// not a negative: the deployment answers absence with a 200 empty form, so a
/// 404 can only be a routing or deployment anomaly and stays Unknown.
pub fn current_validator_status(
    outcome: Option<&str>,
    activity: Option<&str>,
    freshness: &str,
) -> CurrentValidatorStatusView {
    let unknown = || CurrentValidatorStatusView {
        status: CurrentValidatorStatus::Unknown,
        state: "unknown",
        qualifier: None,
    };
    match outcome.unwrap_or("") {
        "success" => {
            let (status, qualifier) = classify_activity(activity);
            if status == CurrentValidatorStatus::Unknown {
                return unknown();
            }
            let state = match freshness {
                "fresh" => "current",
                "stale" => "stale",
                _ => "unknown",
            };
            CurrentValidatorStatusView {
                status,
                state,
                qualifier,
            }
        }
        // Strictly validated 200/code 0/empty nodeId/status 0: authoritative
        // absence of current staking identity.
        "empty" => CurrentValidatorStatusView {
            status: CurrentValidatorStatus::NotValidator,
            state: match freshness {
                "fresh" => "current",
                "stale" => "stale",
                _ => "unknown",
            },
            qualifier: None,
        },
        // A failed refresh with a retained last-good Activity is stale; the
        // verdict is retained but never presented as fresh.
        "error" => {
            let (status, qualifier) = classify_activity(activity);
            if status == CurrentValidatorStatus::Unknown {
                return unknown();
            }
            CurrentValidatorStatusView {
                status,
                state: "stale",
                qualifier,
            }
        }
        _ => unknown(),
    }
}

/// Project only a confirmed verdict, independently of detail metrics and the
/// latest attempt. A failed/partial refresh cannot revive activity superseded by
/// authoritative absence, and ambiguous pre-migration failures remain Unknown.
pub fn project_verdict(
    outcome: &str,
    last_good_outcome: Option<&str>,
    activity: Option<&str>,
    verdict_freshness: &str,
) -> (String, String, CurrentValidatorStatusView) {
    // These responses cannot classify an identity. Retain persisted evidence
    // for a later recoverable refresh, but preserve their Unknown presentation.
    let last_good_outcome = if matches!(outcome, "not_found" | "unsupported" | "not_configured") {
        None
    } else {
        last_good_outcome
    };
    let (evidence_outcome, evidence_activity) = match last_good_outcome {
        Some("empty") => ("empty", Some("observing")),
        Some("success") => ("success", activity),
        _ => ("unknown", None),
    };
    let freshness = if evidence_outcome == "unknown" || verdict_freshness == "unknown" {
        "unknown"
    } else if outcome == "error"
        || outcome == "not_found"
        || outcome == "unsupported"
        || outcome == "not_configured"
    {
        "stale"
    } else {
        verdict_freshness
    };
    let projected_outcome = if freshness == "unknown" {
        "unknown"
    } else {
        evidence_outcome
    };
    let (activity, activity_state) =
        project_activity(projected_outcome, evidence_activity, freshness);
    let status = current_validator_status(Some(projected_outcome), evidence_activity, freshness);
    (activity, activity_state, status)
}

fn platscan_status_activity(status: i64) -> Option<ValidatorActivity> {
    match status {
        1 => Some(ValidatorActivity::Candidate),
        2 => Some(ValidatorActivity::Active),
        3 => Some(ValidatorActivity::Producing),
        4 => Some(ValidatorActivity::Exiting),
        5 => Some(ValidatorActivity::Exited),
        6 => Some(ValidatorActivity::Verifying),
        7 => Some(ValidatorActivity::Locked),
        _ => None,
    }
}

struct RankingPage {
    entries: Vec<(String, i64)>,
    total_count: i64,
}

/// Validate one page of the dedicated `aliveStakingList` ranking response.
/// The upstream returns `RespPage` directly: `code` 0, `totalCount`, and a
/// `data` array whose `ranking` is the global 1-based position, not a
/// page-local index. The expected global position is recomputed from the
/// requested page and row order, so a page-local or drifted rank is a
/// detectable inconsistency rather than an accepted position (#158).
fn normalize_platscan_ranking_response(
    value: &Value,
    page_no: usize,
    page_size: usize,
) -> Result<RankingPage, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "PlatScan ranking response was not an object".to_owned())?;
    let code = object
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| "PlatScan ranking response did not contain a success envelope".to_owned())?;
    if code != 0 {
        return Err("PlatScan returned an unsuccessful ranking envelope".to_owned());
    }
    if let Some(err_msg) = object.get("errMsg") {
        if !err_msg.is_string() {
            return Err("PlatScan returned an invalid ranking envelope message".to_owned());
        }
    }
    let total_count = object
        .get("totalCount")
        .and_then(Value::as_i64)
        .ok_or_else(|| "PlatScan ranking response did not contain a total".to_owned())?;
    if total_count < 0 {
        return Err("PlatScan returned a negative ranking total".to_owned());
    }
    let data = match object.get("data") {
        Some(Value::Array(data)) => data.as_slice(),
        Some(Value::Null) => &[],
        _ => {
            return Err("PlatScan ranking response did not contain a data list".to_owned());
        }
    };
    if data.len() > page_size {
        return Err("PlatScan ranking page exceeded the requested size".to_owned());
    }
    let base = (page_no.saturating_sub(1)) * page_size;
    let mut entries = Vec::with_capacity(data.len());
    for (index, row) in data.iter().enumerate() {
        let row = row
            .as_object()
            .ok_or_else(|| "PlatScan ranking entry was not an object".to_owned())?;
        let node_id = row
            .get("nodeId")
            .and_then(Value::as_str)
            .ok_or_else(|| "PlatScan ranking entry omitted a Validator identifier".to_owned())?;
        if !is_platscan_node_id(node_id) {
            return Err("PlatScan ranking entry had an invalid Validator identifier".to_owned());
        }
        let ranking = row
            .get("ranking")
            .and_then(Value::as_i64)
            .ok_or_else(|| "PlatScan ranking entry omitted a rank".to_owned())?;
        let expected = base as i64 + index as i64 + 1;
        if ranking != expected {
            return Err("PlatScan ranking was not a consistent global sequence".to_owned());
        }
        entries.push((node_id.to_owned(), ranking));
    }
    Ok(RankingPage {
        entries,
        total_count,
    })
}

fn normalize_platscan_response(
    value: &Value,
    requested_node_id: &str,
) -> Result<Option<ValidatorObservation>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "PlatScan response was not an object".to_owned())?;
    let code = object
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| "PlatScan response did not contain a success envelope".to_owned())?;
    if code != 0 {
        return Err("PlatScan returned an unsuccessful envelope".to_owned());
    }
    if let Some(err_msg) = object.get("errMsg") {
        if !err_msg.is_string() {
            return Err("PlatScan returned an invalid envelope message".to_owned());
        }
    }
    let data = object
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| "PlatScan response did not contain a data object".to_owned())?;
    let node_id = data.get("nodeId").and_then(Value::as_str).ok_or_else(|| {
        "PlatScan response did not contain a Validator node identifier".to_owned()
    })?;
    let status = data
        .get("status")
        .and_then(Value::as_i64)
        .ok_or_else(|| "PlatScan response did not contain a Validator status".to_owned())?;
    if node_id.is_empty() && status == 0 {
        return Ok(None);
    }
    if node_id != requested_node_id {
        return Err("PlatScan returned a mismatched Validator node identifier".to_owned());
    }
    let activity = platscan_status_activity(status)
        .ok_or_else(|| "PlatScan returned an unsupported Validator status".to_owned())?;
    let read_string = |names: &[&str]| -> Result<Option<String>, String> {
        let Some(value) = names.iter().find_map(|name| data.get(*name)) else {
            return Ok(None);
        };
        let value = match value.as_str() {
            Some(value) if !value.is_empty() => value.to_owned(),
            Some(_) => return Ok(None),
            None if value.is_null() => return Ok(None),
            // A JSON number with a fractional or exponent part has already
            // been converted to a binary float by serde_json, so its exact
            // source digits are unrecoverable. Accept integral numbers
            // exactly and reject the rest instead of corrupting an amount
            // (#155).
            None if value.is_number() => {
                if let Some(integer) = value.as_i64() {
                    integer.to_string()
                } else if let Some(integer) = value.as_u64() {
                    integer.to_string()
                } else {
                    return Err(
                        "PlatScan returned a numeric amount that cannot be represented exactly"
                            .to_owned(),
                    );
                }
            }
            _ => return Err("PlatScan returned an invalid text value".to_owned()),
        };
        normalize_bounded_text(&value)
    };
    let read_int = |names: &[&str]| -> Result<Option<i64>, String> {
        let Some(value) = names.iter().find_map(|name| data.get(*name)) else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(None);
        }
        if let Some(number) = value.as_i64() {
            return if number >= 0 {
                Ok(Some(number))
            } else {
                Err("PlatScan returned a negative integer".to_owned())
            };
        }
        let text = match value.as_str() {
            Some(text) if !text.is_empty() => text,
            Some(_) => return Ok(None),
            None => return Err("PlatScan returned an invalid integer".to_owned()),
        };
        let parsed = text
            .parse::<i64>()
            .map_err(|_| "PlatScan returned an out-of-range integer".to_owned())?;
        if parsed < 0 {
            return Err("PlatScan returned a negative integer".to_owned());
        }
        Ok(Some(parsed))
    };
    // A percentage may arrive as a bounded decimal string with an optional
    // trailing `%` (the investigated source emits e.g. "90.909091%"), or as a
    // non-negative JSON number. It is normalized to percentage points without
    // the sign so the Public projection and both Node views share one unit
    // (#156). Fractional JSON numbers are accepted here because a rate is not
    // an exact monetary amount; only exponent notation is rejected rather than
    // fabricating digits.
    let read_percentage = |names: &[&str]| -> Result<Option<String>, String> {
        let Some(value) = names.iter().find_map(|name| data.get(*name)) else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(None);
        }
        if let Some(text) = value.as_str() {
            return normalize_percentage_value(text);
        }
        if let Some(integer) = value.as_i64() {
            return normalize_percentage_value(&integer.to_string());
        }
        if let Some(integer) = value.as_u64() {
            return normalize_percentage_value(&integer.to_string());
        }
        if let Some(number) = value.as_f64() {
            if !number.is_finite() || number < 0.0 {
                return Err("PlatScan returned an invalid percentage".to_owned());
            }
            let text = value.to_string();
            if text.contains(['e', 'E']) {
                return Err("PlatScan returned an invalid percentage".to_owned());
            }
            return normalize_percentage_value(&text);
        }
        Err("PlatScan returned an invalid percentage".to_owned())
    };
    // A delegation reward distribution percentage is bounded to 0..=100
    // percentage points. An out-of-range value is rejected rather than
    // clamped or reinterpreted: an unscaled basis-point 2000 must never
    // masquerade as 2000%, and a negative or malformed value is never a
    // valid ratio (#157).
    let read_bounded_percentage = |names: &[&str]| -> Result<Option<String>, String> {
        let Some(value) = read_percentage(names)? else {
            return Ok(None);
        };
        if decimal_exceeds_max(&value, 100) {
            return Err("PlatScan returned an out-of-range percentage".to_owned());
        }
        Ok(Some(value))
    };
    let observation = ValidatorObservation {
        provider_timestamp: None,
        activity: Some(activity),
        stake_amount: read_string(&["stakingValue", "totalValue", "stake"])?,
        reward_amount: read_string(&["rewardValue", "reward"])?,
        reward_rate: read_string(&["deleAnnualizedRate", "rewardRate"])?,
        delegation_reward_percentage: read_bounded_percentage(&["rewardPer"])?,
        delegator_count: read_int(&["delegateQty", "delegatorCount"])?,
        epoch: read_int(&["epoch"])?,
        block_count: read_int(&["blockQty", "blockCount"])?,
        expected_block_count: read_int(&["expectBlockQty", "expectedBlockQty"])?,
        gen_blocks_rate: read_percentage(&["genBlocksRate", "generatedBlocksRate"])?,
    };
    for value in [
        observation.stake_amount.as_deref(),
        observation.reward_amount.as_deref(),
        observation.reward_rate.as_deref(),
        observation.delegation_reward_percentage.as_deref(),
        observation.gen_blocks_rate.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_nonnegative_decimal(value)?;
    }
    Ok(Some(observation))
}

fn normalize_bounded_text(value: &str) -> Result<Option<String>, String> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err("Provider returned an invalid bounded value".to_owned());
    }
    Ok(Some(value.to_owned()))
}

fn normalize_percentage_value(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let unsigned = value.strip_suffix('%').unwrap_or(value).trim();
    if unsigned.is_empty() {
        return Err("PlatScan returned an invalid percentage".to_owned());
    }
    validate_nonnegative_decimal(unsigned)
        .map_err(|_| "PlatScan returned an invalid percentage".to_owned())?;
    Ok(Some(unsigned.to_owned()))
}

fn validate_nonnegative_decimal(value: &str) -> Result<(), String> {
    let mut dots = 0;
    let mut digits = 0;
    for character in value.chars() {
        match character {
            '0'..='9' => digits += 1,
            '.' => dots += 1,
            _ => return Err("Provider returned an invalid numeric value".to_owned()),
        }
    }
    if digits == 0 || dots > 1 {
        return Err("Provider returned an invalid numeric value".to_owned());
    }
    Ok(())
}

/// One parsed bounded nonnegative decimal: a normalized integer part and the
/// source's exact fractional digits. Trailing fractional zeros are part of the
/// value and are preserved, because the aggregate must not invent or discard
/// source precision.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DecimalAmount {
    integer: String,
    fractional: String,
}

/// Parse nonnegative decimal syntax only: at least one digit, at most one dot.
/// It deliberately carries no length bound, because it is also used to re-read
/// an accumulated total, which can legitimately grow past any single source
/// value length by exactly the carry it needs to stay exact.
fn parse_decimal_digits(value: &str) -> Option<DecimalAmount> {
    let mut dots = 0;
    let mut digits = 0;
    for character in value.chars() {
        match character {
            '0'..='9' => digits += 1,
            '.' => dots += 1,
            _ => return None,
        }
    }
    if digits == 0 || dots > 1 {
        return None;
    }
    let (whole, fractional) = value.split_once('.').unwrap_or((value, ""));
    Some(DecimalAmount {
        integer: whole.trim_start_matches('0').to_owned(),
        fractional: fractional.to_owned(),
    })
}

/// Parse one bounded source decimal without touching binary floating point.
/// Accepts the same syntax as the trust boundary: at least one digit, at most
/// one dot, bounded length, and no control characters.
fn parse_decimal_amount(value: &str) -> Option<DecimalAmount> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return None;
    }
    parse_decimal_digits(value)
}

/// Render a parsed amount, treating an all-zero integer part as `0`.
fn render_decimal_amount(amount: &DecimalAmount) -> String {
    let integer = if amount.integer.is_empty() {
        "0"
    } else {
        amount.integer.as_str()
    };
    if amount.fractional.is_empty() {
        integer.to_owned()
    } else {
        format!("{integer}.{}", amount.fractional)
    }
}

/// Left-pad an amount's integer part and right-pad its fractional part so two
/// amounts can be added digit-by-digit at a shared scale.
fn scaled_decimal_digits(
    amount: &DecimalAmount,
    integer_width: usize,
    fraction_width: usize,
) -> String {
    let mut digits = String::with_capacity(integer_width + fraction_width);
    for _ in amount.integer.len()..integer_width {
        digits.push('0');
    }
    digits.push_str(&amount.integer);
    digits.push_str(&amount.fractional);
    for _ in amount.fractional.len()..fraction_width {
        digits.push('0');
    }
    digits
}

/// Add two equal-length digit strings and return the (possibly one digit
/// longer) base-10 sum.
fn add_decimal_digits(left: &str, right: &str) -> String {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut result = vec![0_u8; left.len() + 1];
    let mut carry = 0_u8;
    for index in (0..left.len()).rev() {
        let sum = (left[index] - b'0') + (right[index] - b'0') + carry;
        result[index + 1] = b'0' + (sum % 10);
        carry = sum / 10;
    }
    result[0] = b'0' + carry;
    String::from_utf8(result).expect("decimal digits are ASCII")
}

/// Fold one bounded nonnegative decimal string into an exact running total.
///
/// Provider amounts cross the trust boundary as source-precision decimal
/// strings and must never round-trip through binary floating point, so the
/// aggregate is plain string arithmetic. `total` starts as `None` (no known
/// value) and only becomes `Some` for an accepted value. The return value
/// reports whether `value` was accepted, so the caller can keep its coverage
/// and stale counts exact instead of guessing from the total.
pub fn accumulate_decimal(total: &mut Option<String>, value: &str) -> bool {
    let Some(incoming) = parse_decimal_amount(value) else {
        return false;
    };
    let Some(current) = total.as_deref() else {
        *total = Some(render_decimal_amount(&incoming));
        return true;
    };
    // The running total is always produced by render_decimal_amount, so it is
    // valid by construction; it is deliberately not re-subjected to the
    // per-source length bound, which would panic once the carry exceeds 256
    // digits.
    let current = parse_decimal_digits(current)
        .expect("an accumulated total is always a valid nonnegative decimal");
    let integer_width = current.integer.len().max(incoming.integer.len());
    let fraction_width = current.fractional.len().max(incoming.fractional.len());
    let left = scaled_decimal_digits(&current, integer_width, fraction_width);
    let right = scaled_decimal_digits(&incoming, integer_width, fraction_width);
    let sum = add_decimal_digits(&left, &right);
    let split = sum.len() - fraction_width;
    *total = Some(render_decimal_amount(&DecimalAmount {
        integer: sum[..split].trim_start_matches('0').to_owned(),
        fractional: sum[split..].to_owned(),
    }));
    true
}

/// Whether a validated nonnegative decimal string is greater than `max`.
/// The integer part is compared directly, so a percentage never round-trips
/// through a binary float.
fn decimal_exceeds_max(value: &str, max: u64) -> bool {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u64>().unwrap_or(u64::MAX)
    };
    whole > max || (whole == max && fraction.bytes().any(|byte| byte != b'0'))
}

fn provider_diagnostic(value: String) -> String {
    let value = crate::redaction::redact_sensitive(&value)
        .replace("https://", "[redacted-url]/")
        .replace("http://", "[redacted-url]/");
    value.chars().take(MAX_PROVIDER_DIAGNOSTIC_LEN).collect()
}

fn observation_key(observation: &ValidatorObservation) -> String {
    let bytes = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        observation.provider_timestamp,
        observation.activity,
        observation.stake_amount,
        observation.reward_amount,
        observation.reward_rate,
        observation.delegation_reward_percentage,
        observation.delegator_count,
        observation.epoch,
        observation.block_count,
        observation.expected_block_count,
        observation.gen_blocks_rate
    );
    let mut hash = Sha256::new();
    hash.update(bytes.as_bytes());
    format!("{:x}", hash.finalize())
}

/// Cumulative actual / cumulative scheduled blocks from one successful
/// observation (#156). The returned percentage string carries no `%` sign and
/// is rounded half-up to six decimal places, mirroring the source's own rate
/// precision. Never combine a fresh numerator with an older denominator: both
/// inputs come from the same stored row.
///
/// * A `Some(0)` denominator is `not_applicable` — no scheduled duties —
///   regardless of whether the numerator is known, and is deliberately not
///   rendered as 0% or 100%.
/// * A missing half of the pair is `unknown`, never a synthesized zero.
/// * `ok` is the only state that carries a value.
pub fn cumulative_block_rate(
    block_count: Option<i64>,
    expected_block_count: Option<i64>,
) -> (Option<String>, &'static str) {
    match (block_count, expected_block_count) {
        (_, Some(0)) => (None, "not_applicable"),
        (Some(block_count), Some(expected_block_count)) => (
            Some(completion_rate_percentage(
                block_count,
                expected_block_count,
            )),
            "ok",
        ),
        _ => (None, "unknown"),
    }
}

/// Scale used by `completion_rate_percentage`: six fractional decimal places.
const RATE_SCALE: i128 = 1_000_000;

/// Render `numerator / denominator × 100` as a percentage string without the
/// `%` sign. Integer arithmetic only: the ratio never round-trips through a
/// binary float, and trailing zeros beyond the source's integer precision are
/// trimmed.
fn completion_rate_percentage(numerator: i64, denominator: i64) -> String {
    debug_assert!(denominator > 0);
    let numerator = i128::from(numerator.max(0));
    let denominator = i128::from(denominator);
    let scaled = (numerator * 100 * RATE_SCALE * 2 + denominator) / (denominator * 2);
    let integer = scaled / RATE_SCALE;
    let fraction = scaled % RATE_SCALE;
    if fraction == 0 {
        return integer.to_string();
    }
    let mut fraction = format!("{fraction:06}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{integer}.{fraction}")
}

fn decimal_decreased(previous: Option<&str>, current: Option<&str>) -> bool {
    let Some((previous, current)) = previous.zip(current) else {
        return false;
    };
    let normalize = |value: &str| {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        let whole = whole.trim_start_matches('0');
        let whole = if whole.is_empty() { "0" } else { whole };
        let fraction = fraction.trim_end_matches('0');
        (whole.to_owned(), fraction.to_owned())
    };
    let (previous_whole, previous_fraction) = normalize(previous);
    let (current_whole, current_fraction) = normalize(current);
    previous_whole.len() > current_whole.len()
        || (previous_whole.len() == current_whole.len()
            && (previous_whole > current_whole
                || (previous_whole == current_whole && {
                    let width = previous_fraction.len().max(current_fraction.len());
                    let previous_fraction = format!("{:0<width$}", previous_fraction);
                    let current_fraction = format!("{:0<width$}", current_fraction);
                    previous_fraction > current_fraction
                })))
}

fn counter_decreases(
    existing: Option<&ValidatorInsightRecord>,
    observation: &ValidatorObservation,
) -> Vec<(&'static str, String, String)> {
    let Some(existing) = existing else {
        return Vec::new();
    };
    let mut decreases = Vec::new();
    if decimal_decreased(
        existing.stake_amount.as_deref(),
        observation.stake_amount.as_deref(),
    ) {
        decreases.push((
            "stake_amount",
            existing.stake_amount.clone().unwrap_or_default(),
            observation.stake_amount.clone().unwrap_or_default(),
        ));
    }
    if decimal_decreased(
        existing.reward_amount.as_deref(),
        observation.reward_amount.as_deref(),
    ) {
        decreases.push((
            "reward_amount",
            existing.reward_amount.clone().unwrap_or_default(),
            observation.reward_amount.clone().unwrap_or_default(),
        ));
    }
    if matches!(
        (existing.block_count, observation.block_count),
        (Some(previous), Some(current)) if current < previous
    ) {
        decreases.push((
            "block_count",
            existing.block_count.unwrap_or_default().to_string(),
            observation.block_count.unwrap_or_default().to_string(),
        ));
    }
    decreases
}

#[derive(Debug, Clone, FromRow)]
pub struct ValidatorRecord {
    pub validator_id: String,
    pub network_key: String,
    pub validator_node_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct NodeValidatorLinkRecord {
    pub link_id: String,
    pub node_id: String,
    pub validator_id: String,
    /// Legacy manual role. Automatic Links never carry a role (#173).
    pub role: Option<String>,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

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

pub fn validate_validator_node_id(value: &str) -> Result<(), ValidatorError> {
    if value.is_empty()
        || value.chars().count() > MAX_VALIDATOR_NODE_ID_LEN
        || value.chars().any(|character| character.is_control())
    {
        return Err(ValidatorError::InvalidValidatorNodeId);
    }
    Ok(())
}

pub fn validate_display_name(value: Option<&str>) -> Result<(), ValidatorError> {
    if let Some(value) = value {
        if value.is_empty()
            || value.chars().count() > MAX_VALIDATOR_DISPLAY_NAME_LEN
            || value.chars().any(|character| character.is_control())
        {
            return Err(ValidatorError::InvalidDisplayName);
        }
    }
    Ok(())
}

pub fn validate_role(value: &str) -> Result<(), ValidatorError> {
    matches!(value, "primary" | "standby" | "observer")
        .then_some(())
        .ok_or(ValidatorError::InvalidRole)
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime, ValidatorError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|error| ValidatorError::InvalidTimestamp(error.to_string()))
}

fn canonical_timestamp(value: &str) -> Result<String, ValidatorError> {
    Ok(format_rfc3339(parse_timestamp(value)?))
}

fn canonical_validity(
    valid_from: &str,
    valid_until: Option<&str>,
) -> Result<(String, Option<String>), ValidatorError> {
    validate_validity(valid_from, valid_until)?;
    Ok((
        canonical_timestamp(valid_from)?,
        valid_until.map(canonical_timestamp).transpose()?,
    ))
}

pub fn validate_validity(
    valid_from: &str,
    valid_until: Option<&str>,
) -> Result<(), ValidatorError> {
    let from = parse_timestamp(valid_from)?;
    if let Some(until) = valid_until {
        if parse_timestamp(until)? <= from {
            return Err(ValidatorError::InvalidValidity);
        }
    }
    Ok(())
}

pub async fn create_validator(
    db: &ServerDatabase,
    network_key: &str,
    validator_node_id: &str,
    display_name: Option<&str>,
    actor_user_id: &str,
) -> Result<(ValidatorRecord, i64), ValidatorError> {
    validate_validator_node_id(validator_node_id)?;
    validate_display_name(display_name)?;
    let now = format_rfc3339(now_utc());
    let validator_id = uuid::Uuid::new_v4().to_string();
    let mut tx = db.pool().begin().await?;

    let known: Option<i64> = sqlx::query_scalar("SELECT 1 FROM networks WHERE network_key = ?")
        .bind(network_key)
        .fetch_optional(&mut *tx)
        .await?;
    if known.is_none() {
        return Err(ValidatorError::NetworkNotFound);
    }

    let insert = sqlx::query(
        "INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&validator_id)
    .bind(network_key)
    .bind(validator_node_id)
    .bind(display_name)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert {
        if error
            .as_database_error()
            .is_some_and(|database_error| database_error.is_unique_violation())
        {
            return Err(ValidatorError::ValidatorAlreadyExists);
        }
        return Err(ValidatorError::Database(error));
    }

    insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        "validator_created",
        "validator",
        &validator_id,
        Some(&serde_json::json!({
            "network_key": network_key,
            "validator_node_id": validator_node_id,
            "display_name": display_name,
        })),
    )
    .await?;
    let audit_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        ValidatorRecord {
            validator_id,
            network_key: network_key.to_owned(),
            validator_node_id: validator_node_id.to_owned(),
            display_name: display_name.map(str::to_owned),
            created_at: now.clone(),
            updated_at: now,
        },
        audit_id,
    ))
}

pub async fn get_validator(
    db: &ServerDatabase,
    validator_id: &str,
) -> Result<Option<ValidatorRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorRecord>(
        "SELECT validator_id, network_key, validator_node_id, display_name, created_at, updated_at FROM validators WHERE validator_id = ?",
    )
    .bind(validator_id)
    .fetch_optional(db.pool())
    .await?)
}

pub async fn list_validators(
    db: &ServerDatabase,
    network_key: Option<&str>,
) -> Result<Vec<ValidatorRecord>, ValidatorError> {
    let rows = if let Some(network_key) = network_key {
        sqlx::query_as::<_, ValidatorRecord>(
            "SELECT validator_id, network_key, validator_node_id, display_name, created_at, updated_at FROM validators WHERE network_key = ? ORDER BY validator_node_id, validator_id",
        )
        .bind(network_key)
        .fetch_all(db.pool())
        .await?
    } else {
        sqlx::query_as::<_, ValidatorRecord>(
            "SELECT validator_id, network_key, validator_node_id, display_name, created_at, updated_at FROM validators ORDER BY network_key, validator_node_id, validator_id",
        )
        .fetch_all(db.pool())
        .await?
    };
    Ok(rows)
}

async fn link_overlaps(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    valid_from: &str,
    valid_until: Option<&str>,
    exclude_link_id: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let mut query = String::from(
        "SELECT 1 FROM node_validator_links WHERE node_id = ? AND (? < COALESCE(valid_until, '9999-12-31T23:59:59Z')) AND (valid_until IS NULL OR valid_from < ?)",
    );
    if exclude_link_id.is_some() {
        query.push_str(" AND link_id != ?");
    }
    query.push_str(" LIMIT 1");
    let mut statement = sqlx::query_scalar::<_, i64>(&query)
        .bind(node_id)
        .bind(valid_from)
        .bind(valid_until.unwrap_or("9999-12-31T23:59:59Z"));
    if let Some(exclude_link_id) = exclude_link_id {
        statement = statement.bind(exclude_link_id);
    }
    Ok(statement.fetch_optional(&mut **tx).await?.is_some())
}

async fn validate_link_parentage(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    validator_id: &str,
) -> Result<(), ValidatorError> {
    let node = sqlx::query_as::<_, (String, String)>(
        "SELECT network_key, lifecycle FROM nodes WHERE node_id = ?",
    )
    .bind(node_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((node_network, lifecycle)) = node else {
        return Err(ValidatorError::NodeNotFound);
    };
    if lifecycle != "active" {
        return Err(ValidatorError::NodeNotActive);
    }
    let validator_network = sqlx::query_scalar::<_, String>(
        "SELECT network_key FROM validators WHERE validator_id = ?",
    )
    .bind(validator_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(validator_network) = validator_network else {
        return Err(ValidatorError::ValidatorNotFound);
    };
    if node_network != validator_network {
        return Err(ValidatorError::NetworkMismatch);
    }
    Ok(())
}

fn is_overlap_database_error(error: &sqlx::Error) -> bool {
    error.to_string().contains("node_validator_link_overlap")
}

pub async fn create_link(
    db: &ServerDatabase,
    node_id: &str,
    validator_id: &str,
    role: &str,
    valid_from: &str,
    valid_until: Option<&str>,
    actor_user_id: &str,
) -> Result<(NodeValidatorLinkRecord, i64), ValidatorError> {
    validate_role(role)?;
    let (valid_from, valid_until) = canonical_validity(valid_from, valid_until)?;
    let now = format_rfc3339(now_utc());
    let link_id = uuid::Uuid::new_v4().to_string();
    let mut tx = db.pool().begin().await?;
    validate_link_parentage(&mut tx, node_id, validator_id).await?;
    if link_overlaps(&mut tx, node_id, &valid_from, valid_until.as_deref(), None).await? {
        return Err(ValidatorError::LinkOverlap);
    }
    let insert = sqlx::query(
        "INSERT INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&link_id)
    .bind(node_id)
    .bind(validator_id)
    .bind(role)
    .bind(&valid_from)
    .bind(valid_until.as_deref())
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert {
        if is_overlap_database_error(&error) {
            return Err(ValidatorError::LinkOverlap);
        }
        return Err(ValidatorError::Database(error));
    }
    insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        "node_validator_link_created",
        "node_validator_link",
        &link_id,
        Some(&serde_json::json!({
            "node_id": node_id,
            "validator_id": validator_id,
            "role": role,
            "valid_from": valid_from,
            "valid_until": valid_until,
        })),
    )
    .await?;
    let audit_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        NodeValidatorLinkRecord {
            link_id,
            node_id: node_id.to_owned(),
            validator_id: validator_id.to_owned(),
            role: Some(role.to_owned()),
            valid_from: valid_from.to_owned(),
            valid_until,

            created_at: now.clone(),
            updated_at: now,
        },
        audit_id,
    ))
}

pub async fn get_link(
    db: &ServerDatabase,
    link_id: &str,
) -> Result<Option<NodeValidatorLinkRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, NodeValidatorLinkRecord>(
        "SELECT link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at FROM node_validator_links WHERE link_id = ?",
    )
    .bind(link_id)
    .fetch_optional(db.pool())
    .await?)
}

pub async fn list_links(
    db: &ServerDatabase,
    node_id: Option<&str>,
    validator_id: Option<&str>,
    network_key: Option<&str>,
) -> Result<Vec<NodeValidatorLinkRecord>, ValidatorError> {
    let mut sql = String::from(
        "SELECT l.link_id, l.node_id, l.validator_id, l.role, l.valid_from, l.valid_until, l.created_at, l.updated_at FROM node_validator_links l JOIN validators v ON v.validator_id = l.validator_id JOIN nodes n ON n.node_id = l.node_id WHERE 1=1",
    );
    if node_id.is_some() {
        sql.push_str(" AND l.node_id = ?");
    }
    if validator_id.is_some() {
        sql.push_str(" AND l.validator_id = ?");
    }
    if network_key.is_some() {
        sql.push_str(" AND v.network_key = ?");
    }
    sql.push_str(" ORDER BY l.valid_from DESC, l.link_id DESC");
    let mut query = sqlx::query_as::<_, NodeValidatorLinkRecord>(&sql);
    if let Some(value) = node_id {
        query = query.bind(value);
    }
    if let Some(value) = validator_id {
        query = query.bind(value);
    }
    if let Some(value) = network_key {
        query = query.bind(value);
    }
    Ok(query.fetch_all(db.pool()).await?)
}

pub async fn update_link(
    db: &ServerDatabase,
    link_id: &str,
    role: &str,
    valid_from: &str,
    valid_until: Option<&str>,
    actor_user_id: &str,
) -> Result<(NodeValidatorLinkRecord, i64), ValidatorError> {
    validate_role(role)?;
    let (valid_from, valid_until) = canonical_validity(valid_from, valid_until)?;
    let now = format_rfc3339(now_utc());
    let mut tx = db.pool().begin().await?;
    let existing = sqlx::query_as::<_, (String, String, String, String, Option<String>)>(
        "SELECT node_id, validator_id, role, valid_from, valid_until FROM node_validator_links WHERE link_id = ?",
    )
    .bind(link_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((node_id, validator_id, _old_role, old_from, old_until)) = existing else {
        return Err(ValidatorError::LinkNotFound);
    };
    if old_until.is_some() {
        return Err(ValidatorError::LinkAlreadyEnded);
    }
    validate_link_parentage(&mut tx, &node_id, &validator_id).await?;
    if parse_timestamp(&valid_from)? <= parse_timestamp(&old_from)? {
        return Err(ValidatorError::LinkReplacementMustAdvance);
    }
    if link_overlaps(
        &mut tx,
        &node_id,
        &valid_from,
        valid_until.as_deref(),
        Some(link_id),
    )
    .await?
    {
        return Err(ValidatorError::LinkOverlap);
    }
    sqlx::query(
        "UPDATE node_validator_links SET valid_until = ?, updated_at = ? WHERE link_id = ?",
    )
    .bind(&valid_from)
    .bind(&now)
    .bind(link_id)
    .execute(&mut *tx)
    .await?;
    let replacement_id = uuid::Uuid::new_v4().to_string();
    let insert = sqlx::query(
        "INSERT INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&replacement_id)
    .bind(&node_id)
    .bind(&validator_id)
    .bind(role)
    .bind(&valid_from)
    .bind(valid_until.as_deref())
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert {
        if is_overlap_database_error(&error) {
            return Err(ValidatorError::LinkOverlap);
        }
        return Err(ValidatorError::Database(error));
    }
    insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        "node_validator_link_replaced",
        "node_validator_link",
        link_id,
        Some(&serde_json::json!({
            "replacement_link_id": replacement_id,
            "node_id": node_id,
            "validator_id": validator_id,
            "role": role,
            "valid_from": valid_from,
            "valid_until": valid_until,
        })),
    )
    .await?;
    let audit_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    let row = sqlx::query_as::<_, NodeValidatorLinkRecord>(
        "SELECT link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at FROM node_validator_links WHERE link_id = ?",
    )
    .bind(&replacement_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((row, audit_id))
}

pub async fn end_link(
    db: &ServerDatabase,
    link_id: &str,
    ended_at: Option<&str>,
    actor_user_id: &str,
) -> Result<(NodeValidatorLinkRecord, i64), ValidatorError> {
    let now = format_rfc3339(now_utc());
    let end = canonical_timestamp(ended_at.unwrap_or(&now))?;
    let end_time = parse_timestamp(&end)?;
    let mut tx = db.pool().begin().await?;
    let valid_from = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT valid_from, valid_until FROM node_validator_links WHERE link_id = ?",
    )
    .bind(link_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((valid_from, valid_until)) = valid_from else {
        return Err(ValidatorError::LinkNotFound);
    };
    if valid_until.is_some() {
        return Err(ValidatorError::LinkAlreadyEnded);
    }
    if end_time <= parse_timestamp(&valid_from)? {
        return Err(ValidatorError::EndBeforeStart);
    }
    sqlx::query(
        "UPDATE node_validator_links SET valid_until = CASE WHEN valid_until IS NULL OR valid_until > ? THEN ? ELSE valid_until END, updated_at = ? WHERE link_id = ?",
    )
    .bind(&end)
    .bind(&end)
    .bind(&now)
    .bind(link_id)
    .execute(&mut *tx)
    .await?;
    insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        "node_validator_link_ended",
        "node_validator_link",
        link_id,
        Some(&serde_json::json!({ "valid_until": end })),
    )
    .await?;
    let audit_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    let row = sqlx::query_as::<_, NodeValidatorLinkRecord>(
        "SELECT link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at FROM node_validator_links WHERE link_id = ?",
    )
    .bind(link_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((row, audit_id))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorInsightRecord {
    pub validator_id: String,
    pub source: Option<String>,
    pub outcome: String,
    pub diagnostic: Option<String>,
    pub provider_timestamp: Option<String>,
    pub activity: Option<String>,
    pub last_attempt_received_at: String,
    pub last_good_received_at: Option<String>,
    pub last_good_provider_timestamp: Option<String>,
    pub last_good_verdict_outcome: Option<String>,
    pub last_good_verdict_received_at: Option<String>,
    pub rank: Option<i64>,
    pub rank_outcome: Option<String>,
    pub rank_diagnostic: Option<String>,
    pub rank_last_attempt_received_at: Option<String>,
    pub rank_last_good_received_at: Option<String>,
    pub rank_cohort_size: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegation_reward_percentage: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
    pub expected_block_count: Option<i64>,
    pub gen_blocks_rate: Option<String>,
    pub counter_state: String,
    pub change_state: String,
    pub candidate_previous_rank: Option<i64>,
    pub candidate_rank: Option<i64>,
    pub candidate_observations: i64,
    pub candidate_observed_at: Option<String>,
    pub candidate_provider_timestamp: Option<String>,
    pub candidate_observation_key: Option<String>,
    pub last_observation_key: Option<String>,
    pub updated_at: String,
}

/// The canonical column list for loading one current Validator insight. Kept in
/// one place so the detail apply, the ranking apply, and direct loads cannot
/// drift apart.
const INSIGHT_SELECT: &str = "SELECT validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, last_good_verdict_outcome, last_good_verdict_received_at, rank, rank_outcome, rank_diagnostic, rank_last_attempt_received_at, rank_last_good_received_at, rank_cohort_size, stake_amount, reward_amount, reward_rate, delegation_reward_percentage, delegator_count, epoch, block_count, expected_block_count, gen_blocks_rate, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at FROM current_validator_insights WHERE validator_id = ?";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorRankingHistoryRecord {
    pub history_id: String,
    pub validator_id: String,
    pub previous_rank: Option<i64>,
    pub current_rank: i64,
    pub observed_at: String,
    pub provider_timestamp: Option<String>,
    pub observation_key: String,
    pub candidate_observed_at: Option<String>,
    pub candidate_provider_timestamp: Option<String>,
    pub candidate_observation_key: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorCounterHistoryRecord {
    pub history_id: String,
    pub validator_id: String,
    pub counter_name: String,
    pub previous_value: String,
    pub current_value: String,
    pub observed_at: String,
    pub provider_timestamp: Option<String>,
    pub observation_key: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorLinkContextRecord {
    pub link_id: String,
    pub node_id: String,
    pub role: Option<String>,
    pub valid_from: String,
    pub valid_until: Option<String>,
}

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
        "SELECT snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count FROM validator_daily_snapshots WHERE validator_id = ? ORDER BY sample_at DESC, local_date DESC LIMIT ?",
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
        "SELECT aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max, rank_last, stake_last, reward_last, reward_rate_last, delegator_count_last, epoch_last, block_count_last, updated_at FROM validator_monthly_aggregates WHERE validator_id = ? ORDER BY month_key DESC, timezone LIMIT ?",
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
        "SELECT l.link_id, l.node_id, l.origin, l.valid_from, l.valid_until, n.display_name AS node_display_name, n.lifecycle AS node_lifecycle FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.validator_id = ? ORDER BY l.valid_from DESC, l.link_id DESC LIMIT ?",
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
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deleted_nodes WHERE network_key = ?")
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
            "SELECT snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count FROM validator_daily_snapshots WHERE validator_id = ? AND timezone = ? AND local_date >= ? AND local_date <= ? ORDER BY local_date DESC LIMIT ?",
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
            "SELECT timezone, COUNT(*) FROM validator_daily_snapshots WHERE validator_id = ? AND timezone != ? AND local_date >= ? AND local_date <= ? GROUP BY timezone ORDER BY timezone",
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
struct StoredDay {
    local_date: String,
    month_key: String,
}

/// Store one detail reading on its configured local day, replacing the stored
/// row only when this reading is newer. Returns whether the write changed
/// stored rows together with the day it was addressed to, so the caller can
/// place same-cycle ranking evidence on exactly that day (#219).
async fn record_daily_snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    validator_id: &str,
    source: &str,
    observation: &ValidatorObservation,
    observation_key: &str,
    received_at: &str,
    timezone: &str,
) -> Result<(bool, StoredDay), ValidatorError> {
    let (local_date, month_key, sample_at) = analytics_period(observation, received_at, timezone)?;
    let result = sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(validator_id, timezone, local_date) DO UPDATE SET snapshot_id=excluded.snapshot_id, month_key=excluded.month_key, sample_at=excluded.sample_at, received_at=excluded.received_at, provider_timestamp=excluded.provider_timestamp, source=excluded.source, observation_key=excluded.observation_key, stake_amount=excluded.stake_amount, reward_amount=excluded.reward_amount, reward_rate=excluded.reward_rate, delegator_count=excluded.delegator_count, epoch=excluded.epoch, block_count=excluded.block_count WHERE excluded.sample_at > validator_daily_snapshots.sample_at OR (excluded.sample_at = validator_daily_snapshots.sample_at AND excluded.observation_key > validator_daily_snapshots.observation_key)")
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
async fn record_snapshot_rank(
    tx: &mut Transaction<'_, Sqlite>,
    validator_id: &str,
    timezone: &str,
    rank: Option<i64>,
    day: &StoredDay,
    now: &str,
) -> Result<(), ValidatorError> {
    let stored = sqlx::query(
        "UPDATE validator_daily_snapshots SET rank = ? WHERE validator_id = ? AND timezone = ? AND local_date = ?",
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

async fn rebuild_monthly_aggregate(
    tx: &mut Transaction<'_, Sqlite>,
    validator_id: &str,
    timezone: &str,
    month_key: &str,
    updated_at: &str,
) -> Result<(), ValidatorError> {
    let summary = sqlx::query_as::<_, (i64, String, String, Option<i64>, Option<i64>)>(
        "SELECT COUNT(*), MIN(sample_at), MAX(sample_at), MIN(rank), MAX(rank) FROM validator_daily_snapshots WHERE validator_id = ? AND timezone = ? AND month_key = ?",
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
        "SELECT rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, NULL FROM validator_daily_snapshots WHERE validator_id = ? AND timezone = ? AND month_key = ? ORDER BY sample_at DESC, observation_key DESC LIMIT 1",
    )
    .bind(validator_id)
    .bind(timezone)
    .bind(month_key)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO validator_monthly_aggregates (aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max, rank_last, stake_last, reward_last, reward_rate_last, delegator_count_last, epoch_last, block_count_last, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(validator_id, timezone, month_key) DO UPDATE SET snapshot_count=excluded.snapshot_count, first_sample_at=excluded.first_sample_at, last_sample_at=excluded.last_sample_at, rank_min=excluded.rank_min, rank_max=excluded.rank_max, rank_last=excluded.rank_last, stake_last=excluded.stake_last, reward_last=excluded.reward_last, reward_rate_last=excluded.reward_rate_last, delegator_count_last=excluded.delegator_count_last, epoch_last=excluded.epoch_last, block_count_last=excluded.block_count_last, updated_at=excluded.updated_at")
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
pub async fn list_ranking_history(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorRankingHistoryRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorRankingHistoryRecord>(
        "SELECT history_id, validator_id, previous_rank, current_rank, observed_at, provider_timestamp, observation_key, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key FROM validator_ranking_history WHERE validator_id = ? ORDER BY observed_at DESC, history_id DESC LIMIT ?",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

pub async fn list_counter_history(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorCounterHistoryRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorCounterHistoryRecord>(
        "SELECT history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key FROM validator_counter_history WHERE validator_id = ? ORDER BY observed_at DESC, history_id DESC LIMIT ?",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

pub async fn list_link_context_at(
    db: &ServerDatabase,
    validator_id: &str,
    observed_at: &str,
    active_only: bool,
) -> Result<Vec<ValidatorLinkContextRecord>, ValidatorError> {
    let mut sql = String::from(
        "SELECT l.link_id, l.node_id, l.role, l.valid_from, l.valid_until FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.validator_id = ? AND l.valid_from <= ? AND (l.valid_until IS NULL OR l.valid_until > ?)",
    );
    if active_only {
        sql.push_str(" AND n.lifecycle = 'active'");
    }
    sql.push_str(" ORDER BY l.node_id, l.link_id");
    Ok(sqlx::query_as::<_, ValidatorLinkContextRecord>(&sql)
        .bind(validator_id)
        .bind(observed_at)
        .bind(observed_at)
        .fetch_all(db.pool())
        .await?)
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshSummary {
    pub attempted: usize,
    pub successful: usize,
    pub changed: usize,
    pub invalidations: usize,
    pub alert_invalidations: usize,
    pub invalidated_network_keys: Vec<String>,
    pub invalidated_validator_ids: Vec<String>,
}

pub async fn load_insight(
    db: &ServerDatabase,
    validator_id: &str,
) -> Result<Option<ValidatorInsightRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorInsightRecord>(INSIGHT_SELECT)
        .bind(validator_id)
        .fetch_optional(db.pool())
        .await?)
}

pub async fn list_insights(
    db: &ServerDatabase,
    network_key: Option<&str>,
) -> Result<Vec<ValidatorInsightRecord>, ValidatorError> {
    let mut sql = String::from(
        "SELECT i.validator_id, i.source, i.outcome, i.diagnostic, i.provider_timestamp, i.activity, i.last_attempt_received_at, i.last_good_received_at, i.last_good_provider_timestamp, i.last_good_verdict_outcome, i.last_good_verdict_received_at, i.rank, i.rank_outcome, i.rank_diagnostic, i.rank_last_attempt_received_at, i.rank_last_good_received_at, i.rank_cohort_size, i.stake_amount, i.reward_amount, i.reward_rate, i.delegation_reward_percentage, i.delegator_count, i.epoch, i.block_count, i.expected_block_count, i.gen_blocks_rate, i.counter_state, i.change_state, i.candidate_previous_rank, i.candidate_rank, i.candidate_observations, i.candidate_observed_at, i.candidate_provider_timestamp, i.candidate_observation_key, i.last_observation_key, i.updated_at FROM current_validator_insights i JOIN validators v ON v.validator_id = i.validator_id",
    );
    if network_key.is_some() {
        sql.push_str(" WHERE v.network_key = ?");
    }
    sql.push_str(" ORDER BY v.network_key, v.validator_node_id, i.validator_id");
    let query = sqlx::query_as::<_, ValidatorInsightRecord>(&sql);
    let rows = if let Some(network_key) = network_key {
        query.bind(network_key).fetch_all(db.pool()).await?
    } else {
        query.fetch_all(db.pool()).await?
    };
    Ok(rows)
}

/// Age of the last successful Provider observation in whole seconds, or `None`
/// when no last-good value was ever recorded. The Server owns this arithmetic
/// so every projection reports last-good age the same way, and it is never
/// rendered as 0 for a Validator that has never had a successful refresh
/// (#218).
pub fn last_good_age_seconds(
    last_good_received_at: Option<&str>,
    now: OffsetDateTime,
) -> Option<i64> {
    let received_at = last_good_received_at.and_then(crate::auth::parse_rfc3339)?;
    Some((now - received_at).whole_seconds().max(0))
}

/// One Node's Server-recorded automatic Validator identity: the discovery
/// evidence state (#173) plus the currently open automatic Link interval.
/// Nothing here is a manual role, an ownership claim, or a consensus-membership
/// statement.
#[derive(Debug, Clone, FromRow)]
pub struct NodeValidatorIdentityRecord {
    pub node_id: String,
    pub node_display_name: Option<String>,
    pub network_key: String,
    pub lifecycle: String,
    /// Discovery state of the last evaluation; `None` when never evaluated.
    pub state: Option<String>,
    /// Full P2P public key currently observed from the Node, when identified.
    pub observed_node_key: Option<String>,
    /// When the discovery dimension last evaluated this Node.
    pub evaluated_at: Option<String>,
    /// The Validator of the currently open automatic Link interval, if any.
    pub validator_id: Option<String>,
    /// That Validator's own chain identity key.
    pub validator_node_key: Option<String>,
}

impl NodeValidatorIdentityRecord {
    /// Whether the open interval is effective for the Public projection. The
    /// interval is Server-recorded evidence on its own; Public additionally
    /// requires the Node to be Active, so an inactive Node keeps its identity
    /// history without a projected association (#173, #218).
    pub fn association_effective(&self) -> bool {
        self.validator_id.is_some() && self.lifecycle == "active"
    }
}

/// List each Node's automatic Validator identity coverage, optionally narrowed
/// to one Node. The open automatic interval is selected with the same temporal
/// predicate the Public projection uses, so Admin never reports a
/// correspondence that Public would not, or the reverse.
pub async fn list_node_validator_identities(
    db: &ServerDatabase,
    node_id: Option<&str>,
) -> Result<Vec<NodeValidatorIdentityRecord>, ValidatorError> {
    let now = format_rfc3339(now_utc());
    let mut sql = String::from(
        "SELECT n.node_id, n.display_name AS node_display_name, n.network_key, n.lifecycle, s.state, s.observed_node_key, s.updated_at AS evaluated_at, l.validator_id, v.validator_node_id AS validator_node_key FROM nodes n LEFT JOIN node_validator_identity_status s ON s.node_id = n.node_id LEFT JOIN node_validator_links l ON l.link_id = (SELECT l2.link_id FROM node_validator_links l2 WHERE l2.node_id = n.node_id AND l2.origin = 'automatic' AND l2.valid_from <= ? AND (l2.valid_until IS NULL OR l2.valid_until > ?) ORDER BY l2.valid_from DESC, l2.link_id DESC LIMIT 1) LEFT JOIN validators v ON v.validator_id = l.validator_id",
    );
    if node_id.is_some() {
        sql.push_str(" WHERE n.node_id = ?");
    }
    sql.push_str(" ORDER BY n.network_key, n.node_id");
    let query = sqlx::query_as::<_, NodeValidatorIdentityRecord>(&sql)
        .bind(&now)
        .bind(&now);
    let rows = if let Some(node_id) = node_id {
        query.bind(node_id).fetch_all(db.pool()).await?
    } else {
        query.fetch_all(db.pool()).await?
    };
    Ok(rows)
}

/// Freshness follows the configured Provider refresh interval rather than a
/// fixed constant: a last-good observation is fresh while it is no older
/// than `stale_after_seconds`, which defaults to two refresh intervals
/// (120s at the 60s default refresh). This keeps a legitimate slower refresh
/// setting from immediately looking stale (#154).
pub fn freshness(
    last_good_received_at: Option<&str>,
    now: OffsetDateTime,
    stale_after_seconds: i64,
) -> &'static str {
    let Some(received_at) = last_good_received_at.and_then(crate::auth::parse_rfc3339) else {
        return "unknown";
    };
    if (now - received_at).whole_seconds().abs() <= stale_after_seconds.max(1) {
        "fresh"
    } else {
        "stale"
    }
}

pub async fn refresh_all(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
) -> Result<RefreshSummary, ValidatorError> {
    refresh_all_with_channels_in_timezone(
        db,
        provider,
        &crate::config::NotificationChannels::default(),
        "UTC",
    )
    .await
}

pub async fn refresh_all_with_channels(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
    channels: &crate::config::NotificationChannels,
) -> Result<RefreshSummary, ValidatorError> {
    refresh_all_with_channels_in_timezone(db, provider, channels, "UTC").await
}

pub async fn refresh_all_with_channels_in_timezone(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
    channels: &crate::config::NotificationChannels,
    timezone: &str,
) -> Result<RefreshSummary, ValidatorError> {
    timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(timezone.to_owned()))?;
    let validators = sqlx::query_as::<_, (String, String, String)>(
        "SELECT validator_id, network_key, validator_node_id FROM validators ORDER BY validator_id",
    )
    .fetch_all(db.pool())
    .await?;
    let mut network_keys: Vec<String> = Vec::new();
    let mut provider_results = Vec::with_capacity(validators.len());
    for (validator_id, network_key, validator_node_id) in validators {
        let result = provider.fetch(&network_key, &validator_node_id).await;
        if !network_keys.contains(&network_key) {
            network_keys.push(network_key.clone());
        }
        provider_results.push((validator_id, network_key, validator_node_id, result));
    }

    // The dedicated ranking list is shared by every Validator on a Network:
    // it is fetched once per distinct Network, never once per monitored Node,
    // and it is not a fallback triggered by a detail failure (#158).
    let mut rankings: BTreeMap<String, RankingProviderResult> = BTreeMap::new();
    for network_key in &network_keys {
        let result = provider.fetch_ranking(network_key).await;
        rankings.insert(network_key.clone(), result);
    }

    let mut summary = RefreshSummary {
        attempted: provider_results.len(),
        ..RefreshSummary::default()
    };
    let mut tx = db.pool().begin().await?;
    for (validator_id, network_key, validator_node_id, result) in provider_results {
        let detail =
            apply_provider_result(&mut tx, provider.source(), &validator_id, result, timezone)
                .await?;
        // Every Network in this loop was collected above; the defensive arm
        // keeps a missing result a degraded outcome rather than a panic.
        let lookup = match rankings.get(&network_key) {
            Some(result) => ranking_lookup(result, &validator_node_id),
            None => NetworkRankingLookup::Error("ranking was not collected".to_owned()),
        };
        let (ranking_changed, ranking_invalidated) = apply_ranking_result(
            &mut tx,
            &validator_id,
            lookup,
            timezone,
            detail.stored_day.as_ref(),
        )
        .await?;
        let alert_changes = crate::alerts::evaluate_validator_in_transaction(
            &mut tx,
            &validator_id,
            channels,
            crate::auth::now_utc(),
        )
        .await
        .map_err(|error| ValidatorError::Alert(error.to_string()))?;
        if alert_changes > 0 {
            summary.alert_invalidations += alert_changes;
        }
        if detail.stored {
            summary.successful += 1;
        }
        if detail.activity_changed || ranking_changed {
            summary.changed += 1;
        }
        if detail.invalidated || ranking_invalidated {
            summary.invalidations += 1;
            summary.invalidated_network_keys.push(network_key);
            summary.invalidated_validator_ids.push(validator_id);
        }
    }
    tx.commit().await?;
    Ok(summary)
}

/// One Validator's view of the shared Network ranking result. The Network
/// list is fetched once and looked up per Validator; an absent identifier is
/// only `Unranked` when the whole list was retrieved completely (#158).
#[derive(Debug, Clone, PartialEq, Eq)]
enum NetworkRankingLookup {
    Ranked { rank: i64, cohort_size: i64 },
    Unranked { cohort_size: i64 },
    NotConfigured(String),
    Unsupported(String),
    Error(String),
}

/// The pending ranking-change candidate used only to debounce the
/// `validator.ranking_changed` alert. The displayed rank is adopted
/// immediately, so this state never delays what the API or the UI shows (#158).
#[derive(Debug, Clone, Default)]
struct RankingCandidate {
    previous_rank: Option<i64>,
    rank: Option<i64>,
    observations: i64,
    observed_at: Option<String>,
    provider_timestamp: Option<String>,
    observation_key: Option<String>,
}

/// One Active Node's automatic-identity inputs: the observed Network Identity
/// and enode plus the registered Network Identity they must match. The full P2P
/// public key is only ever derived from the observed enode (#173).
#[derive(Debug, FromRow)]
struct IdentityCandidateRow {
    node_id: String,
    network_key: String,
    network_genesis_hash: Option<String>,
    network_chain_id: Option<i64>,
    network_p2p_network_id: Option<i64>,
    network_address_hrp: Option<String>,
    enode: Option<String>,
    registered_genesis_hash: String,
    registered_chain_id: i64,
    registered_p2p_network_id: i64,
    registered_address_hrp: String,
}

/// Result of one automatic-identity discovery pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IdentityDiscoverySummary {
    /// Active Nodes examined.
    pub considered: usize,
    /// Nodes with a matching registered Network Identity and a full key.
    pub identified: usize,
    /// Automatic Link intervals opened by this pass.
    pub newly_linked: usize,
    /// Open automatic Link intervals closed because the chain key changed.
    pub closed_intervals: usize,
    /// Nodes left unidentified, each with a persisted reason.
    pub unidentified: usize,
}

impl IdentityDiscoverySummary {
    /// Whether this pass opened or closed an automatic Link, so a surface that
    /// projects the automatic model has something new to read. A pass that only
    /// re-confirmed the state it had already stored changed nothing a page shows.
    pub fn changed_associations(&self) -> bool {
        self.newly_linked > 0 || self.closed_intervals > 0
    }
}

/// Identify Node Validator Links automatically from each Active Node's
/// validated Network and observed full P2P public key (#173, main design
/// §15.4, ADR 0005).
///
/// The Server is the only writer of the automatic model. A Node whose observed
/// Network Identity is missing or does not match its registered Network, or
/// whose enode is absent or malformed, is recorded with an explicit reason and
/// is never guessed into a correspondence. A changed chain key closes the
/// previous interval before the new one opens; the two Validators' cumulative
/// values are never spliced. Legacy manual Links are never used as a fallback:
/// their rows are left for the one-time migration (#174).
pub async fn discover_automatic_links(
    db: &ServerDatabase,
) -> Result<IdentityDiscoverySummary, ValidatorError> {
    let now = format_rfc3339(now_utc());
    let mut tx = db.pool().begin().await?;
    let rows = sqlx::query_as::<_, IdentityCandidateRow>(
        "SELECT n.node_id, n.network_key, c.network_genesis_hash, c.network_chain_id, c.network_p2p_network_id, c.network_address_hrp, c.enode, r.genesis_hash AS registered_genesis_hash, r.chain_id AS registered_chain_id, r.p2p_network_id AS registered_p2p_network_id, r.address_hrp AS registered_address_hrp FROM nodes n JOIN networks r ON r.network_key = n.network_key LEFT JOIN current_node_chain_observations c ON c.node_id = n.node_id WHERE n.lifecycle = 'active' ORDER BY n.node_id",
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut summary = IdentityDiscoverySummary {
        considered: rows.len(),
        ..IdentityDiscoverySummary::default()
    };
    for row in rows {
        let identity_observed = row.network_genesis_hash.is_some()
            && row.network_chain_id.is_some()
            && row.network_p2p_network_id.is_some();
        let identity_matches = row.network_genesis_hash.as_deref()
            == Some(row.registered_genesis_hash.as_str())
            && row.network_chain_id == Some(row.registered_chain_id)
            && row.network_p2p_network_id == Some(row.registered_p2p_network_id)
            // The address HRP is optional Network Identity evidence: an absent
            // value is not a mismatch, but a present value that disagrees is.
            && row
                .network_address_hrp
                .as_deref()
                .is_none_or(|hrp| hrp == row.registered_address_hrp.as_str());
        let (state, observed_key) = if !identity_observed {
            ("network_identity_missing", None)
        } else if !identity_matches {
            ("network_identity_mismatch", None)
        } else {
            match row.enode.as_deref() {
                None => ("missing_public_key", None),
                Some(enode) => match observed_p2p_public_key(enode) {
                    Ok(key) => ("identified", Some(key)),
                    Err(_) => ("invalid_public_key", None),
                },
            }
        };

        if state == "identified" {
            let key = observed_key
                .as_deref()
                .expect("an identified Node always has an observed key");
            sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (?, ?, ?, NULL, ?, ?) ON CONFLICT (network_key, validator_node_id) DO NOTHING")
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&row.network_key)
                .bind(key)
                .bind(&now)
                .bind(&now)
                .execute(&mut *tx)
                .await?;
            let validator_id: String = sqlx::query_scalar(
                "SELECT validator_id FROM validators WHERE network_key = ? AND validator_node_id = ?",
            )
            .bind(&row.network_key)
            .bind(key)
            .fetch_one(&mut *tx)
            .await?;

            let open: Option<(String, String, String)> = sqlx::query_as(
                "SELECT link_id, validator_id, valid_from FROM node_validator_links WHERE node_id = ? AND origin = 'automatic' AND (valid_until IS NULL OR valid_until > ?) ORDER BY valid_from DESC, link_id DESC LIMIT 1",
            )
            .bind(&row.node_id)
            .bind(&now)
            .fetch_optional(&mut *tx)
            .await?;

            match open {
                Some((_, current, _)) if current == validator_id => {}
                Some((link_id, _, valid_from)) => {
                    // Close the old interval strictly before opening the new
                    // one, so the overlap trigger and the CHECK hold even when
                    // both happen inside the same second.
                    let close_at = close_interval_boundary(&now, &valid_from)?;
                    sqlx::query(
                        "UPDATE node_validator_links SET valid_until = ?, updated_at = ? WHERE link_id = ?",
                    )
                    .bind(&close_at)
                    .bind(&now)
                    .bind(&link_id)
                    .execute(&mut *tx)
                    .await?;
                    summary.closed_intervals += 1;
                    insert_automatic_link(&mut tx, &row.node_id, &validator_id, &close_at, &now)
                        .await?;
                    summary.newly_linked += 1;
                }
                None => {
                    insert_automatic_link(&mut tx, &row.node_id, &validator_id, &now, &now).await?;
                    summary.newly_linked += 1;
                }
            }
            summary.identified += 1;
        } else {
            // Contradictory evidence ends the previous association interval so
            // Public never keeps projecting a Validator the Node's observed
            // identity no longer supports. Absence of evidence (a missing
            // observation or key) is not contradiction: the last established
            // association is retained and marked stale by Provider freshness.
            if matches!(state, "network_identity_mismatch" | "invalid_public_key") {
                let open: Option<(String, String)> = sqlx::query_as(
                    "SELECT link_id, valid_from FROM node_validator_links WHERE node_id = ? AND origin = 'automatic' AND (valid_until IS NULL OR valid_until > ?) ORDER BY valid_from DESC, link_id DESC LIMIT 1",
                )
                .bind(&row.node_id)
                .bind(&now)
                .fetch_optional(&mut *tx)
                .await?;
                if let Some((link_id, valid_from)) = open {
                    let close_at = close_interval_boundary(&now, &valid_from)?;
                    sqlx::query(
                        "UPDATE node_validator_links SET valid_until = ?, updated_at = ? WHERE link_id = ?",
                    )
                    .bind(&close_at)
                    .bind(&now)
                    .bind(&link_id)
                    .execute(&mut *tx)
                    .await?;
                    summary.closed_intervals += 1;
                }
            }
            summary.unidentified += 1;
        }

        sqlx::query("INSERT INTO node_validator_identity_status (node_id, state, observed_node_key, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET state = excluded.state, observed_node_key = excluded.observed_node_key, updated_at = excluded.updated_at")
            .bind(&row.node_id)
            .bind(state)
            .bind(observed_key.as_deref())
            .bind(&now)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(summary)
}

/// The boundary at which an old interval closes. It must be strictly after the
/// old interval's valid_from; a same-second key change advances by one second
/// instead of writing an impossible interval.
fn close_interval_boundary(now: &str, valid_from: &str) -> Result<String, ValidatorError> {
    let now = parse_timestamp(now)?;
    let from = parse_timestamp(valid_from)?;
    let boundary = if now > from {
        now
    } else {
        from + time::Duration::seconds(1)
    };
    Ok(format_rfc3339(boundary))
}

async fn insert_automatic_link(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    validator_id: &str,
    valid_from: &str,
    updated_at: &str,
) -> Result<(), ValidatorError> {
    sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES (?, ?, ?, NULL, 'automatic', ?, NULL, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(node_id)
        .bind(validator_id)
        .bind(valid_from)
        .bind(updated_at)
        .bind(updated_at)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn ranking_lookup(result: &RankingProviderResult, validator_node_id: &str) -> NetworkRankingLookup {
    match result {
        RankingProviderResult::Success(ranking) => match ranking.entries.get(validator_node_id) {
            Some(rank) => NetworkRankingLookup::Ranked {
                rank: *rank,
                cohort_size: ranking.cohort_size,
            },
            None => NetworkRankingLookup::Unranked {
                cohort_size: ranking.cohort_size,
            },
        },
        RankingProviderResult::NotConfigured(value) => {
            NetworkRankingLookup::NotConfigured(value.clone())
        }
        RankingProviderResult::Unsupported(value) => {
            NetworkRankingLookup::Unsupported(value.clone())
        }
        RankingProviderResult::Error(value) => NetworkRankingLookup::Error(value.clone()),
    }
}

fn ranking_observation_key(rank: i64, cohort_size: i64) -> String {
    format!("ranking:{rank}/{cohort_size}")
}

/// What one detail read changed, and the configured local day it stored a
/// snapshot for (#219). `stored_day` is `None` when this cycle stored no
/// snapshot at all, which is what keeps same-cycle ranking evidence off days
/// nobody observed.
struct AppliedDetail {
    stored: bool,
    activity_changed: bool,
    invalidated: bool,
    stored_day: Option<StoredDay>,
}

async fn apply_provider_result(
    tx: &mut Transaction<'_, Sqlite>,
    source: &str,
    validator_id: &str,
    result: ValidatorProviderResult,
    timezone: &str,
) -> Result<AppliedDetail, ValidatorError> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let existing = sqlx::query_as::<_, ValidatorInsightRecord>(INSIGHT_SELECT)
        .bind(validator_id)
        .fetch_optional(&mut **tx)
        .await?;
    match result {
        ValidatorProviderResult::Success(observation) => {
            validate_observation(&observation)?;
            let key = observation_key(&observation);
            let stored_activity = match observation.activity {
                Some(activity) => Some(activity.as_str()),
                None => existing.as_ref().and_then(|row| row.activity.as_deref()),
            };
            // A partial metrics success is not new Activity/identity evidence.
            // Retain an absence until an explicit Activity supersedes it.
            let verdict_outcome = if observation.activity.is_some() {
                Some("success")
            } else {
                existing
                    .as_ref()
                    .and_then(|row| row.last_good_verdict_outcome.as_deref())
            };
            let verdict_received_at = if observation.activity.is_some() {
                Some(now.as_str())
            } else {
                existing
                    .as_ref()
                    .and_then(|row| row.last_good_verdict_received_at.as_deref())
            };
            let activity_changed = existing.as_ref().and_then(|row| row.activity.as_deref())
                != stored_activity
                || existing
                    .as_ref()
                    .and_then(|row| row.last_good_verdict_outcome.as_deref())
                    != verdict_outcome;
            // Receipt/attempt recovery changes the displayed evidence even
            // when the Activity value and historical sample are unchanged.
            let verdict_refreshed = existing.as_ref().is_none_or(|row| {
                row.outcome != "success"
                    || row.diagnostic.is_some()
                    || row.last_good_verdict_received_at.as_deref() != verdict_received_at
            });
            if existing
                .as_ref()
                .and_then(|row| row.last_observation_key.as_deref())
                == Some(key.as_str())
            {
                sqlx::query(
                    "UPDATE current_validator_insights SET outcome = 'success', diagnostic = NULL, activity = ?, last_good_verdict_outcome = ?, last_good_verdict_received_at = ?, last_attempt_received_at = ?, last_good_received_at = ?, counter_state = 'normal', updated_at = ? WHERE validator_id = ?",
                )
                .bind(stored_activity)
                .bind(verdict_outcome)
                .bind(verdict_received_at)
                .bind(&now)
                .bind(&now)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
                let (analytics_changed, stored_day) = record_daily_snapshot(
                    tx,
                    validator_id,
                    source,
                    &observation,
                    &key,
                    &now,
                    timezone,
                )
                .await?;
                rebuild_monthly_aggregate(tx, validator_id, timezone, &stored_day.month_key, &now)
                    .await?;
                return Ok(AppliedDetail {
                    stored: true,
                    activity_changed,
                    invalidated: analytics_changed || activity_changed || verdict_refreshed,
                    stored_day: Some(stored_day),
                });
            }

            let decreases = counter_decreases(existing.as_ref(), &observation);
            let counter_changed = !decreases.is_empty();
            let counter_state = if counter_changed {
                "counter_reset"
            } else {
                "normal"
            };

            for (counter_name, previous_value, current_value) in decreases {
                sqlx::query("INSERT OR IGNORE INTO validator_counter_history (history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(validator_id)
                    .bind(counter_name)
                    .bind(previous_value)
                    .bind(current_value)
                    .bind(&now)
                    .bind(observation.provider_timestamp.as_deref())
                    .bind(&key)
                    .execute(&mut **tx)
                    .await?;
            }

            let source = bounded_source(source);
            sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, stake_amount, reward_amount, reward_rate, delegation_reward_percentage, delegator_count, epoch, block_count, expected_block_count, gen_blocks_rate, counter_state, last_observation_key, updated_at) VALUES (?, ?, 'success', NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(validator_id) DO UPDATE SET source=excluded.source, outcome=excluded.outcome, diagnostic=NULL, provider_timestamp=excluded.provider_timestamp, activity=excluded.activity, last_attempt_received_at=excluded.last_attempt_received_at, last_good_received_at=excluded.last_good_received_at, last_good_provider_timestamp=excluded.last_good_provider_timestamp, stake_amount=excluded.stake_amount, reward_amount=excluded.reward_amount, reward_rate=excluded.reward_rate, delegation_reward_percentage=excluded.delegation_reward_percentage, delegator_count=excluded.delegator_count, epoch=excluded.epoch, block_count=excluded.block_count, expected_block_count=excluded.expected_block_count, gen_blocks_rate=excluded.gen_blocks_rate, counter_state=excluded.counter_state, last_observation_key=excluded.last_observation_key, updated_at=excluded.updated_at")
                .bind(validator_id)
                .bind(&source)
                .bind(observation.provider_timestamp.as_deref())
                .bind(stored_activity)
                .bind(&now)
                .bind(&now)
                .bind(observation.provider_timestamp.as_deref())
                .bind(observation.stake_amount.as_deref())
                .bind(observation.reward_amount.as_deref())
                .bind(observation.reward_rate.as_deref())
                .bind(observation.delegation_reward_percentage.as_deref())
                .bind(observation.delegator_count)
                .bind(observation.epoch)
                .bind(observation.block_count)
                .bind(observation.expected_block_count)
                .bind(observation.gen_blocks_rate.as_deref())
                .bind(counter_state)
                .bind(&key)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            sqlx::query("UPDATE current_validator_insights SET last_good_verdict_outcome = ?, last_good_verdict_received_at = ? WHERE validator_id = ?")
                .bind(verdict_outcome).bind(verdict_received_at).bind(validator_id)
                .execute(&mut **tx).await?;
            let (analytics_changed, stored_day) = record_daily_snapshot(
                tx,
                validator_id,
                &source,
                &observation,
                &key,
                &now,
                timezone,
            )
            .await?;
            rebuild_monthly_aggregate(tx, validator_id, timezone, &stored_day.month_key, &now)
                .await?;
            Ok(AppliedDetail {
                stored: true,
                activity_changed,
                invalidated: analytics_changed || activity_changed || verdict_refreshed,
                stored_day: Some(stored_day),
            })
        }
        outcome => {
            let (name, diagnostic) = match outcome {
                ValidatorProviderResult::NotFound => ("not_found", None),
                ValidatorProviderResult::AuthoritativeEmpty => ("empty", None),
                ValidatorProviderResult::NotConfigured(value) => {
                    ("not_configured", Some(provider_diagnostic(value)))
                }
                ValidatorProviderResult::Error(value) => {
                    ("error", Some(provider_diagnostic(value)))
                }
                ValidatorProviderResult::Unsupported(value) => {
                    ("unsupported", Some(provider_diagnostic(value)))
                }
                ValidatorProviderResult::Success(_) => unreachable!(),
            };
            let invalidated = existing.as_ref().is_none_or(|row| {
                row.outcome != name
                    || row.diagnostic != diagnostic
                    || (name == "empty"
                        && (row.last_good_verdict_outcome.as_deref() != Some("empty")
                            || row.last_good_verdict_received_at.as_deref() != Some(now.as_str())))
            });
            let source = bounded_source(source);
            sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, last_attempt_received_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(validator_id) DO UPDATE SET source=excluded.source, outcome=excluded.outcome, diagnostic=excluded.diagnostic, last_attempt_received_at=excluded.last_attempt_received_at, updated_at=excluded.updated_at")
                .bind(validator_id)
                .bind(source)
                .bind(name)
                .bind(diagnostic)
                .bind(&now)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            if name == "empty" {
                sqlx::query("UPDATE current_validator_insights SET last_good_verdict_outcome = 'empty', last_good_verdict_received_at = ? WHERE validator_id = ?")
                    .bind(&now).bind(validator_id).execute(&mut **tx).await?;
            }
            Ok(AppliedDetail {
                stored: false,
                activity_changed: false,
                invalidated,
                stored_day: None,
            })
        }
    }
}

/// Apply one Validator's shared-Network ranking lookup. Ranking state is
/// stored independently from detail metrics: a ranking failure updates only
/// the rank attempt/diagnostic and never clears the detail columns, and a
/// detail failure never clears a last-good rank (#158).
///
/// The answer also reaches that Validator's snapshot for the configured local
/// day the same refresh cycle observed, because a daily rank trend can only
/// describe ranks the ranking endpoint actually reported for a day that was
/// observed (#219). `stored_day` is that day, taken from the cycle's own
/// detail read: a cycle that stored no snapshot writes no rank, so a fresh
/// ranking answer is never attached to an older day's evidence or to a day
/// nobody observed. The day carries the newest rank reading for it: an
/// authoritative "not in the cohort" answer stores NULL, and a failed ranking
/// attempt leaves the stored reading untouched, exactly like the current-state
/// columns above it.
async fn apply_ranking_result(
    tx: &mut Transaction<'_, Sqlite>,
    validator_id: &str,
    lookup: NetworkRankingLookup,
    timezone: &str,
    stored_day: Option<&StoredDay>,
) -> Result<(bool, bool), ValidatorError> {
    let now_instant = crate::auth::now_utc();
    let now = crate::auth::format_rfc3339(now_instant);
    let existing = sqlx::query_as::<_, ValidatorInsightRecord>(INSIGHT_SELECT)
        .bind(validator_id)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(existing) = existing else {
        return Ok((false, false));
    };
    match lookup {
        NetworkRankingLookup::Ranked { rank, cohort_size } => {
            let baseline_exists =
                existing.rank_last_good_received_at.is_some() && existing.rank.is_some();
            // The last confirmed rank a change is compared against. When a
            // change is already pending, the baseline is the value before it;
            // the pending rank is already the adopted upstream value.
            let baseline = match (existing.candidate_rank, existing.candidate_previous_rank) {
                (Some(_), Some(previous)) => Some(previous),
                _ => existing.rank,
            };
            let mut confirmed_ranking_change = false;
            let candidate = if !baseline_exists || baseline == Some(rank) {
                // The first rank, an unchanged rank, or a return to the known
                // baseline: nothing to record.
                RankingCandidate::default()
            } else if existing.candidate_rank == Some(rank)
                && existing.candidate_previous_rank == baseline
                && existing.candidate_observations == 1
            {
                // A consecutive observation of the same new rank confirms the
                // change for the ranking alert. The displayed rank already
                // adopted it on the first observation.
                let previous = baseline.unwrap_or(rank);
                sqlx::query("INSERT OR IGNORE INTO validator_ranking_history (history_id, validator_id, previous_rank, current_rank, observed_at, provider_timestamp, observation_key, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(validator_id)
                    .bind(previous)
                    .bind(rank)
                    .bind(&now)
                    .bind(None::<&str>)
                    .bind(ranking_observation_key(rank, cohort_size))
                    .bind(existing.candidate_observed_at.as_deref())
                    .bind(existing.candidate_provider_timestamp.as_deref())
                    .bind(existing.candidate_observation_key.as_deref())
                    .execute(&mut **tx)
                    .await?;
                confirmed_ranking_change = true;
                RankingCandidate::default()
            } else {
                // First observation of a change: record it as a candidate for
                // the alert while still displaying the upstream rank.
                RankingCandidate {
                    previous_rank: baseline,
                    rank: Some(rank),
                    observations: 1,
                    observed_at: Some(now.clone()),
                    provider_timestamp: None,
                    observation_key: Some(ranking_observation_key(rank, cohort_size)),
                }
            };

            // The upstream rank is adopted verbatim and immediately; pending
            // alert confirmation never delays the displayed position (#158).
            let stored_rank = Some(rank);
            let change_state = if confirmed_ranking_change {
                "ranking_changed"
            } else {
                "normal"
            };
            let invalidated = existing.rank != stored_rank
                || existing.rank_outcome.as_deref() != Some("success")
                || existing.rank_cohort_size != Some(cohort_size)
                || confirmed_ranking_change;
            sqlx::query("UPDATE current_validator_insights SET rank = ?, rank_outcome = 'success', rank_diagnostic = NULL, rank_last_attempt_received_at = ?, rank_last_good_received_at = ?, rank_cohort_size = ?, change_state = ?, candidate_previous_rank = ?, candidate_rank = ?, candidate_observations = ?, candidate_observed_at = ?, candidate_provider_timestamp = ?, candidate_observation_key = ?, updated_at = ? WHERE validator_id = ?")
                .bind(stored_rank)
                .bind(&now)
                .bind(&now)
                .bind(cohort_size)
                .bind(change_state)
                .bind(candidate.previous_rank)
                .bind(candidate.rank)
                .bind(candidate.observations)
                .bind(candidate.observed_at)
                .bind(candidate.provider_timestamp)
                .bind(candidate.observation_key)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
            if let Some(day) = stored_day {
                record_snapshot_rank(tx, validator_id, timezone, stored_rank, day, &now).await?;
            }
            Ok((confirmed_ranking_change, invalidated))
        }
        NetworkRankingLookup::Unranked { cohort_size } => {
            let invalidated = existing.rank.is_some()
                || existing.rank_outcome.as_deref() != Some("success")
                || existing.rank_cohort_size != Some(cohort_size)
                || existing.change_state != "normal";
            sqlx::query("UPDATE current_validator_insights SET rank = NULL, rank_outcome = 'success', rank_diagnostic = NULL, rank_last_attempt_received_at = ?, rank_last_good_received_at = ?, rank_cohort_size = ?, change_state = 'normal', candidate_previous_rank = NULL, candidate_rank = NULL, candidate_observations = 0, candidate_observed_at = NULL, candidate_provider_timestamp = NULL, candidate_observation_key = NULL, updated_at = ? WHERE validator_id = ?")
                .bind(&now)
                .bind(&now)
                .bind(cohort_size)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
            if let Some(day) = stored_day {
                record_snapshot_rank(tx, validator_id, timezone, None, day, &now).await?;
            }
            Ok((false, invalidated))
        }
        failure => {
            let (name, diagnostic) = match failure {
                NetworkRankingLookup::NotConfigured(value) => {
                    ("not_configured", Some(provider_diagnostic(value)))
                }
                NetworkRankingLookup::Unsupported(value) => {
                    ("unsupported", Some(provider_diagnostic(value)))
                }
                NetworkRankingLookup::Error(value) => ("error", Some(provider_diagnostic(value))),
                NetworkRankingLookup::Ranked { .. } | NetworkRankingLookup::Unranked { .. } => {
                    unreachable!()
                }
            };
            let invalidated = existing.rank_outcome.as_deref() != Some(name)
                || existing.rank_diagnostic != diagnostic
                || existing.change_state != "normal";
            sqlx::query("UPDATE current_validator_insights SET rank_outcome = ?, rank_diagnostic = ?, rank_last_attempt_received_at = ?, change_state = 'normal', candidate_previous_rank = NULL, candidate_rank = NULL, candidate_observations = 0, candidate_observed_at = NULL, candidate_provider_timestamp = NULL, candidate_observation_key = NULL, updated_at = ? WHERE validator_id = ?")
                .bind(name)
                .bind(diagnostic)
                .bind(&now)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
            Ok((false, invalidated))
        }
    }
}
fn bounded_source(source: &str) -> String {
    source.chars().take(64).collect()
}

fn validate_observation(observation: &ValidatorObservation) -> Result<(), ValidatorError> {
    let has_supported_value = observation.activity.is_some()
        || observation.stake_amount.is_some()
        || observation.reward_amount.is_some()
        || observation.reward_rate.is_some()
        || observation.delegation_reward_percentage.is_some()
        || observation.delegator_count.is_some()
        || observation.epoch.is_some()
        || observation.block_count.is_some()
        || observation.expected_block_count.is_some()
        || observation.gen_blocks_rate.is_some();
    if !has_supported_value {
        return Err(ValidatorError::InvalidProviderObservation(
            "empty observation".to_owned(),
        ));
    }
    if observation.delegator_count.is_some_and(|value| value < 0)
        || observation.epoch.is_some_and(|value| value < 0)
        || observation.block_count.is_some_and(|value| value < 0)
        || observation
            .expected_block_count
            .is_some_and(|value| value < 0)
    {
        return Err(ValidatorError::InvalidProviderObservation(
            "negative integer".to_owned(),
        ));
    }
    for value in [
        observation.stake_amount.as_deref(),
        observation.reward_amount.as_deref(),
        observation.reward_rate.as_deref(),
        observation.delegation_reward_percentage.as_deref(),
        observation.gen_blocks_rate.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        normalize_bounded_text(value).map_err(ValidatorError::InvalidProviderObservation)?;
        validate_nonnegative_decimal(value).map_err(ValidatorError::InvalidProviderObservation)?;
    }
    if observation
        .delegation_reward_percentage
        .as_deref()
        .is_some_and(|value| decimal_exceeds_max(value, 100))
    {
        return Err(ValidatorError::InvalidProviderObservation(
            "out-of-range delegation reward percentage".to_owned(),
        ));
    }
    if let Some(timestamp) = observation.provider_timestamp.as_deref() {
        canonical_timestamp(timestamp)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::database::{ServerDatabaseConfig, initialize};
    use crate::network::create_network;

    use super::*;

    /// A migrator restricted to the given schema version, so a test can build a
    /// historical fixture and apply exactly one forward migration.
    fn migrator_through(version: i64) -> sqlx::migrate::Migrator {
        use std::borrow::Cow;

        sqlx::migrate::Migrator {
            migrations: Cow::Owned(
                crate::database::SERVER_MIGRATOR
                    .iter()
                    .filter(|migration| migration.version <= version)
                    .cloned()
                    .collect(),
            ),
            ignore_missing: false,
            locking: true,
            no_tx: false,
        }
    }

    async fn test_db() -> (tempfile::TempDir, ServerDatabase) {
        let dir = tempdir().unwrap();
        let db = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        create_network(
            &db,
            "platon-mainnet",
            "Mainnet",
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            1,
            1,
            "lat",
        )
        .await
        .unwrap();
        crate::auth::create_owner(
            &db,
            "owner",
            &crate::auth::hash_password(b"validator-test-password").unwrap(),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-1', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, lifecycle, visibility, inventory_revision, first_seen_at, updated_at, rpc_endpoint) VALUES ('node-1', 'agent-1', 'platon-mainnet', 'active', 'private', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 'http://127.0.0.1:1')")
            .execute(db.pool())
            .await
            .unwrap();
        (dir, db)
    }

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
    async fn seed_trend_validator(pool: &sqlx::SqlitePool) {
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
        pool: &sqlx::SqlitePool,
        timezone: &str,
        local_date: &str,
        received_at: &str,
        provider_timestamp: Option<&str>,
    ) {
        sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES (?, 'validator-1', ?, ?, ?, ?, ?, ?, 'platsScan', ?, 12, '1000.000000', '25.000000', '0.05', 40, 5, 900)")
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
        // retained Validator history must stay exactly as it is (#219, §15.4).
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

    #[tokio::test]
    async fn duplicate_identity_and_temporal_overlap_are_rejected() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let duplicate = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap_err();
        assert!(matches!(duplicate, ValidatorError::ValidatorAlreadyExists));
        create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "primary",
            "2025-01-01T00:00:00Z",
            Some("2025-02-01T00:00:00Z"),
            &owner_id,
        )
        .await
        .unwrap();
        let overlap = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "standby",
            "2025-01-15T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap_err();
        assert!(matches!(overlap, ValidatorError::LinkOverlap));
        let second = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "observer",
            "2025-02-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        assert_eq!(second.0.role.as_deref(), Some("observer"));
    }

    #[tokio::test]
    async fn validity_is_canonical_and_roles_can_change() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0x123", None, &owner_id)
            .await
            .unwrap();
        let (link, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "observer",
            "2025-01-01T01:00:00+01:00",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        assert_eq!(link.valid_from, "2025-01-01T00:00:00Z");
        assert_eq!(link.valid_until, None);
        let (updated, _) = update_link(
            &db,
            &link.link_id,
            "primary",
            "2025-01-01T12:00:00Z",
            Some("2025-01-02T00:00:00Z"),
            &owner_id,
        )
        .await
        .unwrap();
        assert_eq!(updated.role.as_deref(), Some("primary"));
        assert_eq!(updated.valid_from, "2025-01-01T12:00:00Z");
        assert_eq!(
            list_links(&db, Some("node-1"), None, None)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[derive(Default)]
    struct FakeProvider {
        results: std::sync::Mutex<Vec<ValidatorProviderResult>>,
        calls: std::sync::Mutex<Vec<(String, String)>>,
        rankings: std::sync::Mutex<Vec<RankingProviderResult>>,
        ranking_calls: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ValidatorProvider for FakeProvider {
        fn source(&self) -> &str {
            "fake"
        }

        async fn fetch(
            &self,
            network_key: &str,
            validator_node_id: &str,
        ) -> ValidatorProviderResult {
            self.calls
                .lock()
                .unwrap()
                .push((network_key.to_owned(), validator_node_id.to_owned()));
            self.results.lock().unwrap().remove(0)
        }

        async fn fetch_ranking(&self, network_key: &str) -> RankingProviderResult {
            self.ranking_calls
                .lock()
                .unwrap()
                .push(network_key.to_owned());
            let mut rankings = self.rankings.lock().unwrap();
            if rankings.is_empty() {
                RankingProviderResult::Unsupported("fake ranking unsupported".to_owned())
            } else {
                rankings.remove(0)
            }
        }
    }

    fn provider_node_id() -> String {
        format!("0x{}", "ab".repeat(64))
    }

    fn deployments(base_url: &str, networks: &[&str]) -> BTreeMap<String, String> {
        networks
            .iter()
            .map(|network| ((*network).to_owned(), base_url.to_owned()))
            .collect()
    }

    fn ranking_with(cohort_size: i64, entries: &[(&str, i64)]) -> RankingProviderResult {
        let entries = entries
            .iter()
            .map(|(node_id, rank)| ((*node_id).to_owned(), *rank))
            .collect();
        RankingProviderResult::Success(Box::new(NetworkRanking {
            entries,
            cohort_size,
        }))
    }

    fn platscan_ranking_response(
        page_no: usize,
        page_size: usize,
        total_count: i64,
        node_ids: &[String],
    ) -> serde_json::Value {
        let base = (page_no - 1) * page_size;
        let data: Vec<serde_json::Value> = node_ids
            .iter()
            .enumerate()
            .map(|(index, node_id)| {
                serde_json::json!({
                    "nodeId": node_id,
                    "ranking": base as i64 + index as i64 + 1,
                })
            })
            .collect();
        serde_json::json!({
            "code": 0,
            "errMsg": "success",
            "totalCount": total_count,
            "displayTotalCount": total_count,
            "totalPages": (total_count + page_size as i64 - 1) / page_size as i64,
            "data": data,
        })
    }

    fn platscan_success(node_id: &str, status: i64) -> serde_json::Value {
        serde_json::json!({ "code": 0, "errMsg": "success", "data": { "nodeId": node_id, "status": status } })
    }

    #[derive(Clone)]
    struct RecordedMockRequest {
        method: String,
        path: String,
        content_type: Option<String>,
        body: String,
    }

    type MockPlatScanResponseQueue =
        std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<(u16, Vec<u8>)>>>;

    #[derive(Clone)]
    struct MockPlatScanState {
        requests: std::sync::Arc<std::sync::Mutex<Vec<RecordedMockRequest>>>,
        responses: MockPlatScanResponseQueue,
        delay_ms: u64,
    }

    async fn mock_platscan_handler(
        axum::extract::State(state): axum::extract::State<MockPlatScanState>,
        request: axum::http::Request<axum::body::Body>,
    ) -> axum::response::Response<String> {
        let method = request.method().to_string();
        let path = request.uri().path().to_string();
        let content_type = request
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body_bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body_bytes).to_string();
        state.requests.lock().unwrap().push(RecordedMockRequest {
            method,
            path,
            content_type,
            body,
        });
        if state.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(state.delay_ms)).await;
        }
        let (status, body) = state
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or((500, Vec::new()));
        axum::response::Response::builder()
            .status(axum::http::StatusCode::from_u16(status).unwrap())
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(String::from_utf8_lossy(&body).to_string())
            .unwrap()
    }

    async fn start_mock_platscan(
        responses: Vec<(u16, Vec<u8>)>,
        delay_ms: u64,
    ) -> (String, MockPlatScanState, tokio::task::JoinHandle<()>) {
        let state = MockPlatScanState {
            requests: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            responses: std::sync::Arc::new(std::sync::Mutex::new(responses.into())),
            delay_ms,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route(
                "/browser-server/staking/stakingDetails",
                axum::routing::post(mock_platscan_handler),
            )
            .route(
                "/browser-server/staking/aliveStakingList",
                axum::routing::post(mock_platscan_handler),
            )
            .with_state(state.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), state, handle)
    }

    /// Replay the unmodified mainnet HTTP bodies, not a synthetic approximation.
    /// Capture requests, hashes, date and pinned source reconciliation are in
    /// tests/fixtures/platscan-mainnet/provenance.json and
    /// docs/design/platscan-mainnet-validation.md. CI never contacts PlatScan.
    #[tokio::test]
    async fn platscan_mainnet_capture_reaches_public_api_and_retains_independent_last_good() {
        use axum::body::to_bytes;
        use axum::extract::State;

        const DETAIL: &[u8] =
            include_bytes!("../tests/fixtures/platscan-mainnet/staking-details.json");
        const PAGES: [&[u8]; 5] = [
            include_bytes!("../tests/fixtures/platscan-mainnet/alive-staking-list-page-1.json"),
            include_bytes!("../tests/fixtures/platscan-mainnet/alive-staking-list-page-2.json"),
            include_bytes!("../tests/fixtures/platscan-mainnet/alive-staking-list-page-3.json"),
            include_bytes!("../tests/fixtures/platscan-mainnet/alive-staking-list-page-4.json"),
            include_bytes!("../tests/fixtures/platscan-mainnet/alive-staking-list-page-5.json"),
        ];
        const NODE_ID: &str = "0xc6c2f9185236d29b3deb0a463b10bf65c88fed993128b422b1f5e1c8fcf7f32e8c8d0a896b3969303c85b4815cf42715c06dfcd33c5b5dc3e78b4159d7f771e2";
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", NODE_ID, None, &owner_id)
            .await
            .unwrap();
        let (link, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "observer",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        // The metrics acceptance path now runs on the automatic model: the
        // legacy manual Link would never be a Public fallback (#173).
        sqlx::query("UPDATE node_validator_links SET origin = 'automatic' WHERE link_id = ?")
            .bind(&link.link_id)
            .execute(db.pool())
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let app_state = crate::http::AppState::new(db, None, auth);

        // Initial success, detail success with failed ranking, then detail failure
        // with successful ranking. Outages are controlled mutations, not captures.
        let mut responses = vec![(200, DETAIL.to_vec())];
        responses.extend(PAGES.iter().map(|body| (200, body.to_vec())));
        responses.extend([(200, DETAIL.to_vec()), (503, Vec::new()), (503, Vec::new())]);
        responses.extend(PAGES.iter().map(|body| (200, body.to_vec())));
        let (base_url, mock, handle) = start_mock_platscan(responses, 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();

        async fn public_network(state: &crate::http::AppState) -> Value {
            let response = crate::http::public::public_networks(State(state.clone())).await;
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            serde_json::from_slice::<Value>(&body).unwrap()[0].clone()
        }
        assert_eq!(
            refresh_all(app_state.db(), &provider)
                .await
                .unwrap()
                .successful,
            1
        );
        let network = public_network(&app_state).await;
        let insight = &network["nodes"][0]["validator"];
        assert_eq!(insight["validatorNodeId"], NODE_ID);
        assert_eq!(insight["currentValidatorStatus"], "validator");
        assert_eq!(insight["currentValidatorStatusState"], "current");
        assert_eq!(insight["source"], "platscan");
        assert_eq!(insight["blockCount"], 1_016_869);
        assert_eq!(insight["expectedBlockCount"], 1_018_630);
        assert_eq!(insight["rewardAmount"], "7893196.068697377541");
        assert_eq!(insight["rewardRate"], "3.58");
        assert_eq!(insight["delegationRewardPercentage"], "90");
        // The deployment genuinely reports >100%; do not cap it or substitute
        // the local historical completion rate for this source-defined metric.
        assert_eq!(insight["genBlocksRate"], "100.1493");
        assert_eq!(insight["blockRate"], "99.827121");
        assert_eq!(insight["blockRateState"], "ok");
        assert_eq!(insight["rank"], 1);
        assert_eq!(insight["rankCohortSize"], 240);
        assert_eq!(insight["rankState"], "ranked");
        assert_eq!(insight["rankFreshness"], "fresh");
        assert_eq!(insight["state"], "fresh");
        assert!(insight["receivedAt"].is_string());
        assert!(insight["rankReceivedAt"].is_string());
        assert!(insight["providerTimestamp"].is_null());
        assert_eq!(network["validatorSummary"]["blocks"]["knownSum"], "1016869");
        assert_eq!(
            network["validatorSummary"]["rewards"]["knownSum"],
            "7893196.068697377541"
        );
        assert_eq!(network["validatorSummary"]["blocks"]["state"], "complete");
        let health_before = network["nodes"][0]["health"].clone();
        {
            let requests = mock.requests.lock().unwrap();
            assert_eq!(requests.len(), 6);
            assert_eq!(requests[0].path, "/browser-server/staking/stakingDetails");
            assert_eq!(
                serde_json::from_str::<Value>(&requests[0].body).unwrap(),
                serde_json::json!({"nodeId": NODE_ID})
            );
            for (index, request) in requests[1..].iter().enumerate() {
                assert_eq!(request.path, "/browser-server/staking/aliveStakingList");
                assert_eq!(
                    serde_json::from_str::<Value>(&request.body).unwrap(),
                    serde_json::json!({"pageNo": index + 1, "pageSize": 50, "queryStatus": "all"})
                );
            }
        }

        refresh_all(app_state.db(), &provider).await.unwrap();
        let network = public_network(&app_state).await;
        let insight = &network["nodes"][0]["validator"];
        assert_eq!(insight["state"], "fresh");
        assert_eq!(insight["rank"], 1);
        assert_eq!(insight["rankState"], "error");
        assert_eq!(insight["rankFreshness"], "stale");
        assert_eq!(insight["rewardAmount"], "7893196.068697377541");
        assert_eq!(network["validatorSummary"]["blocks"]["staleCount"], 0);

        refresh_all(app_state.db(), &provider).await.unwrap();
        let network = public_network(&app_state).await;
        let insight = &network["nodes"][0]["validator"];
        assert_eq!(insight["state"], "error");
        assert_eq!(insight["rankState"], "ranked");
        assert_eq!(insight["rankFreshness"], "fresh");
        assert_eq!(insight["blockCount"], 1_016_869);
        assert_eq!(insight["rewardAmount"], "7893196.068697377541");
        assert_eq!(insight["genBlocksRate"], "100.1493");
        assert_eq!(insight["delegationRewardPercentage"], "90");
        assert_eq!(network["validatorSummary"]["blocks"]["staleCount"], 1);
        assert_eq!(network["validatorSummary"]["rewards"]["staleCount"], 1);
        assert_eq!(network["nodes"][0]["health"], health_before);
        handle.abort();
    }

    /// The same recorded mainnet Validator appears once as status 2 and once as
    /// status 6 — a candidate in a consensus round whose stake is still in
    /// force. Both are currently valid staking identities, so the
    /// consensus-round capture must reach the Public API as `validator`:
    /// reading it as Unknown dropped the Node from the Home Validator filter
    /// precisely while it was taking part in consensus (Owner decision
    /// 2026-10-08; docs/research/platscan-current-validator-status-evidence.md
    /// section 4.2).
    #[tokio::test]
    async fn platscan_consensus_round_capture_is_validator_in_public_api() {
        use axum::body::to_bytes;
        use axum::extract::State;

        const DETAIL: &[u8] = include_bytes!(
            "../tests/fixtures/platscan-validator-status/staking-details-status-6.json"
        );
        const PAGES: [&[u8]; 5] = [
            include_bytes!(
                "../tests/fixtures/platscan-validator-status/alive-staking-list-page-1.json"
            ),
            include_bytes!(
                "../tests/fixtures/platscan-validator-status/alive-staking-list-page-2.json"
            ),
            include_bytes!(
                "../tests/fixtures/platscan-validator-status/alive-staking-list-page-3.json"
            ),
            include_bytes!(
                "../tests/fixtures/platscan-validator-status/alive-staking-list-page-4.json"
            ),
            include_bytes!(
                "../tests/fixtures/platscan-validator-status/alive-staking-list-page-5.json"
            ),
        ];
        const NODE_ID: &str = "0xc6c2f9185236d29b3deb0a463b10bf65c88fed993128b422b1f5e1c8fcf7f32e8c8d0a896b3969303c85b4815cf42715c06dfcd33c5b5dc3e78b4159d7f771e2";
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", NODE_ID, None, &owner_id)
            .await
            .unwrap();
        let (link, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "observer",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE node_validator_links SET origin = 'automatic' WHERE link_id = ?")
            .bind(&link.link_id)
            .execute(db.pool())
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let app_state = crate::http::AppState::new(db, None, auth);

        let mut responses = vec![(200, DETAIL.to_vec())];
        responses.extend(PAGES.iter().map(|body| (200, body.to_vec())));
        let (base_url, _mock, handle) = start_mock_platscan(responses, 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();

        async fn public_network(state: &crate::http::AppState) -> Value {
            let response = crate::http::public::public_networks(State(state.clone())).await;
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            serde_json::from_slice::<Value>(&body).unwrap()[0].clone()
        }
        assert_eq!(
            refresh_all(app_state.db(), &provider)
                .await
                .unwrap()
                .successful,
            1
        );
        let network = public_network(&app_state).await;
        let insight = &network["nodes"][0]["validator"];
        assert_eq!(insight["validatorNodeId"], NODE_ID);
        // Exactly the literal the Home validator filter compares against.
        assert_eq!(insight["currentValidatorStatus"], "validator");
        assert_eq!(insight["currentValidatorStatusState"], "current");
        assert_eq!(insight["currentValidatorStatusQualifier"], Value::Null);
        // The consensus-round presentation stays visible as its own Activity.
        assert_eq!(insight["activity"], "verifying");
        assert_eq!(insight["activityState"], "current");
        assert_eq!(insight["source"], "platscan");
        assert_eq!(insight["state"], "fresh");
        assert_eq!(insight["blockCount"], 1_019_029);
        assert_eq!(insight["expectedBlockCount"], 1_020_790);
        assert_eq!(insight["rewardAmount"], "7906866.977823844245");
        assert_eq!(insight["rewardRate"], "3.72");
        assert_eq!(insight["genBlocksRate"], "101.5625");
        assert_eq!(insight["rank"], 1);
        assert_eq!(insight["rankState"], "ranked");
        assert_eq!(network["validatorSummary"]["blocks"]["knownSum"], "1019029");
        handle.abort();
    }

    #[test]
    fn platscan_normalization_maps_statuses_and_allows_activity_only_snapshots() {
        let node_id = provider_node_id();
        for (status, activity) in [
            (1, ValidatorActivity::Candidate),
            (2, ValidatorActivity::Active),
            (3, ValidatorActivity::Producing),
            (4, ValidatorActivity::Exiting),
            (5, ValidatorActivity::Exited),
            (6, ValidatorActivity::Verifying),
            (7, ValidatorActivity::Locked),
        ] {
            let observation =
                normalize_platscan_response(&platscan_success(&node_id, status), &node_id)
                    .unwrap()
                    .unwrap();
            assert_eq!(observation.activity, Some(activity), "status {status}");
            assert_eq!(observation.stake_amount, None);
        }
        assert_eq!(
            normalize_platscan_response(&platscan_success("", 0), &node_id).unwrap(),
            None
        );
        assert!(normalize_platscan_response(&platscan_success(&node_id, 8), &node_id).is_err());
        assert!(
            normalize_platscan_response(
                &platscan_success(&format!("0x{}", "cd".repeat(64)), 2),
                &node_id
            )
            .is_err()
        );
        assert!(normalize_platscan_response(&serde_json::json!({ "code": 0 }), &node_id).is_err());
        assert!(
            normalize_platscan_response(
                &serde_json::json!({ "code": 0, "data": { "nodeId": node_id, "status": "3" } }),
                &node_id
            )
            .is_err()
        );
        assert!(
            validate_observation(&ValidatorObservation {
                activity: Some(ValidatorActivity::Exiting),
                ..Default::default()
            })
            .is_ok()
        );
        assert!(validate_observation(&ValidatorObservation::default()).is_err());
    }

    #[test]
    fn platscan_reward_value_is_gross_cumulative_not_netted_or_back_computed() {
        let node_id = provider_node_id();
        // The investigated detail response also carries totalDeleReward,
        // rewardPer, and nextRewardPer. The cumulative reward must be the
        // source's gross rewardValue: never netted with the delegator total,
        // never back-computed from the current distribution ratio, and never
        // rounded through binary floating point.
        let body = serde_json::json!({
            "code": 0,
            "errMsg": "success",
            "data": {
                "nodeId": node_id,
                "status": 3,
                "blockQty": 100,
                "expectBlockQty": 110,
                "rewardValue": "1234567.890123456789",
                "totalDeleReward": "999.999",
                "rewardPer": 20,
                "nextRewardPer": 25,
                "deleAnnualizedRate": "0.05"
            }
        });
        let observation = normalize_platscan_response(&body, &node_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            observation.reward_amount.as_deref(),
            Some("1234567.890123456789")
        );
        // The annualized yield stays a separate field; rewardPer is not it.
        assert_eq!(observation.reward_rate.as_deref(), Some("0.05"));
        // rewardPer is the current delegation distribution percentage in
        // percentage points: source 20 is 20%, never 0.20% or 2000%. The
        // pending nextRewardPer (25) is never the current value.
        assert_eq!(
            observation.delegation_reward_percentage.as_deref(),
            Some("20")
        );
        // A fractional JSON number is already a binary float when parsed, so
        // it must degrade to Error instead of silently losing digits. The
        // payload is parsed from wire text to prove the real path.
        let fractional_number: Value = serde_json::from_str(&format!(
            r#"{{"code": 0, "data": {{"nodeId": "{node_id}", "status": 3, "rewardValue": 1234567.890123456789}}}}"#
        ))
        .unwrap();
        assert!(normalize_platscan_response(&fractional_number, &node_id).is_err());
        // ... while an integral JSON number is exact and accepted.
        let integral_number = serde_json::json!({
            "code": 0,
            "data": { "nodeId": node_id, "status": 3, "rewardValue": 100 }
        });
        assert_eq!(
            normalize_platscan_response(&integral_number, &node_id)
                .unwrap()
                .unwrap()
                .reward_amount
                .as_deref(),
            Some("100")
        );
    }

    #[test]
    fn platscan_delegation_reward_percentage_is_unit_correct_and_bounded() {
        let node_id = provider_node_id();
        let parse = |reward_per: Value| {
            normalize_platscan_response(
                &serde_json::json!({
                    "code": 0,
                    "data": { "nodeId": node_id, "status": 3, "rewardPer": reward_per }
                }),
                &node_id,
            )
        };
        // Percentage points, not a fraction and not an unscaled basis-point
        // count: detail rewardPer 20 is 20%.
        for (source, expected) in [
            (serde_json::json!(20), "20"),
            (serde_json::json!("20"), "20"),
            (serde_json::json!("20.5%"), "20.5"),
            (serde_json::json!(0), "0"),
            (serde_json::json!("0"), "0"),
            (serde_json::json!(100), "100"),
            (serde_json::json!("100.00"), "100.00"),
        ] {
            assert_eq!(
                parse(source)
                    .unwrap()
                    .unwrap()
                    .delegation_reward_percentage
                    .as_deref(),
                Some(expected)
            );
        }
        // Out-of-range or malformed values are invalid, never clamped or
        // reinterpreted as a valid ratio.
        for invalid in [
            serde_json::json!(101),
            serde_json::json!("100.5"),
            serde_json::json!(-1),
            serde_json::json!("abc"),
            serde_json::json!("20%%"),
            serde_json::json!(2000),
        ] {
            assert!(
                parse(invalid.clone()).is_err(),
                "invalid rewardPer {invalid} must be rejected"
            );
        }
        // A missing rewardPer is unknown, and a pending nextRewardPer alone
        // never becomes the current effective ratio.
        assert_eq!(
            normalize_platscan_response(
                &serde_json::json!({
                    "code": 0,
                    "data": { "nodeId": node_id, "status": 3, "nextRewardPer": 25 }
                }),
                &node_id,
            )
            .unwrap()
            .unwrap()
            .delegation_reward_percentage,
            None
        );
    }

    #[tokio::test]
    async fn platscan_adapter_maps_exact_gross_reward_through_controlled_http() {
        let node_id = provider_node_id();
        let body = serde_json::to_vec(&serde_json::json!({
            "code": 0,
            "errMsg": "success",
            "data": {
                "nodeId": node_id,
                "status": 3,
                "rewardValue": "1234567.890123456789",
                "totalDeleReward": "1.0",
                "rewardPer": 20
            }
        }))
        .unwrap();
        let (base_url, state, handle) = start_mock_platscan(vec![(200, body)], 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        match provider.fetch("platon-mainnet", &node_id).await {
            ValidatorProviderResult::Success(observation) => assert_eq!(
                observation.reward_amount.as_deref(),
                Some("1234567.890123456789")
            ),
            other => panic!("expected a successful observation, got {other:?}"),
        }
        assert_eq!(state.requests.lock().unwrap().len(), 1);
        handle.abort();
    }

    #[tokio::test]
    async fn platscan_adapter_posts_staking_details_and_maps_every_status() {
        let node_id = provider_node_id();
        let mut responses = Vec::new();
        let mut expected = Vec::new();
        for (status, activity) in [
            (1, ValidatorActivity::Candidate),
            (2, ValidatorActivity::Active),
            (3, ValidatorActivity::Producing),
            (4, ValidatorActivity::Exiting),
            (5, ValidatorActivity::Exited),
            (6, ValidatorActivity::Verifying),
            (7, ValidatorActivity::Locked),
        ] {
            responses.push((
                200,
                serde_json::to_vec(&platscan_success(&node_id, status)).unwrap(),
            ));
            expected.push((status, activity));
        }
        let (base_url, state, handle) = start_mock_platscan(responses, 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        for (status, activity) in expected {
            assert_eq!(
                provider.fetch("platon-mainnet", &node_id).await,
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    activity: Some(activity),
                    ..Default::default()
                })),
                "status {status}"
            );
        }
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 7);
        for request in requests.iter() {
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/browser-server/staking/stakingDetails");
            assert_eq!(request.content_type.as_deref(), Some("application/json"));
            assert_eq!(request.body, format!(r#"{{"nodeId":"{node_id}"}}"#));
        }
        handle.abort();
    }

    #[tokio::test]
    async fn platscan_rejects_invalid_identifiers_and_unbound_networks_without_request() {
        let (base_url, state, handle) = start_mock_platscan(Vec::new(), 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        let valid = provider_node_id();
        let invalid = vec![
            "0x".to_owned(),
            "0x123".to_owned(),
            "0xzz".to_owned(),
            format!("0x{}", "ab".repeat(63)),
            format!("0x{}", "ab".repeat(65)),
        ];
        for id in invalid {
            assert!(matches!(
                provider.fetch("platon-mainnet", &id).await,
                ValidatorProviderResult::Unsupported(_)
            ));
        }
        // An unbound Network is explicitly NotConfigured, never a request to
        // whichever deployment happens to be configured for another Network.
        assert!(matches!(
            provider.fetch("platon-devnet", &valid).await,
            ValidatorProviderResult::NotConfigured(_)
        ));
        assert!(state.requests.lock().unwrap().is_empty());
        handle.abort();
    }

    #[tokio::test]
    async fn platscan_routes_each_network_to_its_own_deployment() {
        let node_id = provider_node_id();
        let (first_url, first_state, first_handle) = start_mock_platscan(
            vec![(
                200,
                serde_json::to_vec(&platscan_success(&node_id, 2)).unwrap(),
            )],
            0,
        )
        .await;
        let (second_url, second_state, second_handle) = start_mock_platscan(
            vec![(
                200,
                serde_json::to_vec(&platscan_success(&node_id, 3)).unwrap(),
            )],
            0,
        )
        .await;
        let deployments = BTreeMap::from([
            ("platon-mainnet".to_owned(), first_url.clone()),
            ("platon-devnet".to_owned(), second_url.clone()),
        ]);
        let provider =
            PlatScanValidatorProvider::new(deployments, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(
            provider.fetch("platon-mainnet", &node_id).await,
            ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                activity: Some(ValidatorActivity::Active),
                ..Default::default()
            }))
        );
        assert_eq!(
            provider.fetch("platon-devnet", &node_id).await,
            ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                activity: Some(ValidatorActivity::Producing),
                ..Default::default()
            }))
        );
        assert_eq!(first_state.requests.lock().unwrap().len(), 1);
        assert_eq!(second_state.requests.lock().unwrap().len(), 1);
        first_handle.abort();
        second_handle.abort();
    }

    #[tokio::test]
    async fn platscan_refresh_routes_networks_and_persists_each_block_count() {
        let (_dir, db) = test_db().await;
        create_network(
            &db,
            "platon-devnet",
            "Devnet",
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            2,
            2,
            "lat",
        )
        .await
        .unwrap();
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        // The same Validator identifier on two Networks must never share data.
        let shared_node_id = provider_node_id();
        let (mainnet, _) =
            create_validator(&db, "platon-mainnet", &shared_node_id, None, &owner_id)
                .await
                .unwrap();
        let (devnet, _) = create_validator(&db, "platon-devnet", &shared_node_id, None, &owner_id)
            .await
            .unwrap();

        let mainnet_body = serde_json::to_vec(&serde_json::json!({
            "code": 0,
            "data": { "nodeId": shared_node_id, "status": 3, "blockQty": 111 }
        }))
        .unwrap();
        let devnet_body = serde_json::to_vec(&serde_json::json!({
            "code": 0,
            "data": { "nodeId": shared_node_id, "status": 3, "blockQty": 222 }
        }))
        .unwrap();
        // Each Network's deployment also serves the dedicated ranking list, so
        // the detail request is followed by exactly one shared list request.
        let ranking_body = serde_json::to_vec(&platscan_ranking_response(
            1,
            RANKING_PAGE_SIZE,
            1,
            std::slice::from_ref(&shared_node_id),
        ))
        .unwrap();
        let (mainnet_url, mainnet_state, mainnet_handle) =
            start_mock_platscan(vec![(200, mainnet_body), (200, ranking_body.clone())], 0).await;
        let (devnet_url, devnet_state, devnet_handle) =
            start_mock_platscan(vec![(200, devnet_body), (200, ranking_body)], 0).await;
        let provider = PlatScanValidatorProvider::new(
            BTreeMap::from([
                ("platon-mainnet".to_owned(), mainnet_url),
                ("platon-devnet".to_owned(), devnet_url),
            ]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();

        let summary = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(summary.attempted, 2);
        assert_eq!(summary.successful, 2);
        let mainnet_insight = load_insight(&db, &mainnet.validator_id)
            .await
            .unwrap()
            .unwrap();
        let devnet_insight = load_insight(&db, &devnet.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mainnet_insight.block_count, Some(111));
        assert_eq!(devnet_insight.block_count, Some(222));
        let mainnet_requests = mainnet_state.requests.lock().unwrap().clone();
        let devnet_requests = devnet_state.requests.lock().unwrap().clone();
        assert_eq!(mainnet_requests.len(), 2);
        assert_eq!(devnet_requests.len(), 2);
        assert_eq!(
            mainnet_requests[0].path,
            "/browser-server/staking/stakingDetails"
        );
        assert_eq!(
            mainnet_requests[1].path,
            "/browser-server/staking/aliveStakingList"
        );
        mainnet_handle.abort();
        devnet_handle.abort();
    }

    #[tokio::test]
    async fn platscan_authoritative_empty_and_http_404_stays_non_negative() {
        let node_id = provider_node_id();
        let responses = vec![
            (
                200,
                serde_json::to_vec(
                    &serde_json::json!({ "code": 0, "data": { "nodeId": "", "status": 0 } }),
                )
                .unwrap(),
            ),
            (404, Vec::new()),
        ];
        let (base_url, _state, handle) = start_mock_platscan(responses, 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            provider.fetch("platon-mainnet", &node_id).await,
            ValidatorProviderResult::AuthoritativeEmpty
        );
        // A 404 is a routing/deployment anomaly, not an absence: it degrades
        // to an Error so Current Validator Status stays Unknown (#168).
        assert!(matches!(
            provider.fetch("platon-mainnet", &node_id).await,
            ValidatorProviderResult::Error(_)
        ));
        handle.abort();
    }

    async fn negative_verdict_test_state(
        node_id: &str,
    ) -> (tempfile::TempDir, crate::http::AppState) {
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", node_id, None, &owner_id)
            .await
            .unwrap();
        let (link, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "observer",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE node_validator_links SET origin = 'automatic' WHERE link_id = ?")
            .bind(&link.link_id)
            .execute(db.pool())
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        (dir, crate::http::AppState::new(db, None, auth))
    }

    async fn negative_verdict_public_insight(state: &crate::http::AppState) -> Value {
        let response =
            crate::http::public::public_networks(axum::extract::State(state.clone())).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice::<Value>(&body).unwrap()[0]["nodes"][0]["validator"].clone()
    }

    #[tokio::test]
    async fn platscan_negative_verdict_survives_failure_without_reviving_old_activity() {
        // Both positive->absence->failure and first absence->failure use the
        // actual HTTP adapter, transactional refresh and public projection.
        for previously_active in [true, false] {
            let node_id = provider_node_id();
            let (_dir, state) = negative_verdict_test_state(&node_id).await;
            let mut responses = Vec::new();
            if previously_active {
                responses.extend([
                    (
                        200,
                        serde_json::to_vec(&platscan_success(&node_id, 2)).unwrap(),
                    ),
                    (503, Vec::new()),
                ]);
            }
            responses.extend([
                (
                    200,
                    serde_json::to_vec(&serde_json::json!({"code": 0,
                    "data": {"nodeId": "", "status": 0}}))
                    .unwrap(),
                ),
                (503, Vec::new()),
                (503, Vec::new()),
                (503, Vec::new()),
            ]);
            let (base_url, mock, handle) = start_mock_platscan(responses, 0).await;
            let provider = PlatScanValidatorProvider::new(
                deployments(&base_url, &["platon-mainnet"]),
                std::time::Duration::from_secs(5),
            )
            .unwrap();
            if previously_active {
                refresh_all(state.db(), &provider).await.unwrap();
                assert_eq!(
                    negative_verdict_public_insight(&state).await["currentValidatorStatus"],
                    "validator"
                );
                // Age only the old detail metrics. A subsequent absence must
                // not turn these retained amounts/counters into fresh values.
                sqlx::query("UPDATE current_validator_insights SET last_good_received_at = '2025-01-01T00:00:00Z'")
                    .execute(state.db().pool()).await.unwrap();
            }
            refresh_all(state.db(), &provider).await.unwrap();
            let absent = negative_verdict_public_insight(&state).await;
            assert_eq!(absent["currentValidatorStatus"], "not_validator");
            assert_eq!(absent["currentValidatorStatusState"], "current");
            assert_eq!(absent["activity"], "observing");
            assert_eq!(absent["activityState"], "current");
            let metrics_received_at = absent["receivedAt"].clone();
            assert_eq!(
                absent["freshness"],
                if previously_active {
                    "stale"
                } else {
                    "unknown"
                }
            );
            refresh_all(state.db(), &provider).await.unwrap();
            let failed = negative_verdict_public_insight(&state).await;
            assert_eq!(failed["state"], "error");
            assert_eq!(
                failed["currentValidatorStatus"], "not_validator",
                "failed refresh must retain the latest authoritative absence, not resurrect older activity"
            );
            assert_eq!(failed["currentValidatorStatusState"], "stale");
            assert_eq!(failed["activity"], "observing");
            assert_eq!(failed["activityState"], "stale");
            assert_eq!(failed["receivedAt"], metrics_received_at);
            assert_eq!(failed["activityReceivedAt"], absent["activityReceivedAt"]);
            let snapshot_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM validator_daily_snapshots")
                    .fetch_one(state.db().pool())
                    .await
                    .unwrap();
            assert_eq!(
                snapshot_count,
                i64::from(previously_active),
                "absence and failure must not fabricate or rewrite metrics history"
            );
            assert_eq!(
                mock.requests.lock().unwrap().len(),
                if previously_active { 6 } else { 4 }
            );
            handle.abort();
        }
    }

    #[tokio::test]
    async fn platscan_negative_verdict_ages_without_a_new_refresh() {
        let node_id = provider_node_id();
        let (_dir, state) = negative_verdict_test_state(&node_id).await;
        let (base_url, _mock, handle) = start_mock_platscan(
            vec![
                (
                    200,
                    serde_json::to_vec(&serde_json::json!({"code": 0,
                "data": {"nodeId": "", "status": 0}}))
                    .unwrap(),
                ),
                (503, Vec::new()),
            ],
            0,
        )
        .await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        refresh_all(state.db(), &provider).await.unwrap();
        assert_eq!(
            negative_verdict_public_insight(&state).await["currentValidatorStatusState"],
            "current"
        );
        sqlx::query("UPDATE current_validator_insights SET last_attempt_received_at = '2025-01-01T00:00:00Z', last_good_verdict_received_at = '2025-01-01T00:00:00Z'")
            .execute(state.db().pool()).await.unwrap();
        let aged = negative_verdict_public_insight(&state).await;
        assert_eq!(aged["currentValidatorStatus"], "not_validator");
        assert_eq!(
            aged["currentValidatorStatusState"], "stale",
            "confirmed absence must expire just like positive evidence"
        );
        assert_eq!(aged["activity"], "observing");
        assert_eq!(aged["activityState"], "stale");
        handle.abort();
    }

    #[tokio::test]
    async fn platscan_negative_verdict_renewal_invalidates_even_with_unchanged_ranking() {
        let node_id = provider_node_id();
        let (_dir, state) = negative_verdict_test_state(&node_id).await;
        let absent = serde_json::to_vec(&serde_json::json!({"code": 0,
            "data": {"nodeId": "", "status": 0}}))
        .unwrap();
        let (base_url, _mock, handle) = start_mock_platscan(
            vec![
                (200, absent.clone()),
                (503, Vec::new()),
                (200, absent),
                (503, Vec::new()),
            ],
            0,
        )
        .await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        refresh_all(state.db(), &provider).await.unwrap();
        sqlx::query("UPDATE current_validator_insights SET last_good_verdict_received_at = '2025-01-01T00:00:00Z'")
            .execute(state.db().pool()).await.unwrap();
        assert_eq!(
            negative_verdict_public_insight(&state).await["currentValidatorStatusState"],
            "stale"
        );
        let renewed = refresh_all(state.db(), &provider).await.unwrap();
        assert_eq!(
            negative_verdict_public_insight(&state).await["currentValidatorStatusState"],
            "current"
        );
        assert!(
            !renewed.invalidated_validator_ids.is_empty(),
            "a renewed negative verdict must refetch an already-open stale projection"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn confirmed_verdict_replay_recovers_and_invalidates_without_rewriting_history() {
        for interruption in [
            Some(ValidatorProviderResult::Error("failed".to_owned())),
            Some(ValidatorProviderResult::AuthoritativeEmpty),
            None,
        ] {
            let node_id = provider_node_id();
            let (_dir, state) = negative_verdict_test_state(&node_id).await;
            let observation = ValidatorObservation {
                provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                activity: Some(ValidatorActivity::Active),
                stake_amount: Some("123456789000000000000".to_owned()),
                ..Default::default()
            };
            let mut results = vec![ValidatorProviderResult::Success(Box::new(
                observation.clone(),
            ))];
            if let Some(result) = interruption {
                results.push(result);
            }
            results.push(ValidatorProviderResult::Success(Box::new(observation)));
            let provider = FakeProvider {
                results: std::sync::Mutex::new(results),
                ..Default::default()
            };
            refresh_all(state.db(), &provider).await.unwrap();
            if provider.results.lock().unwrap().len() > 1 {
                refresh_all(state.db(), &provider).await.unwrap();
            }
            sqlx::query("UPDATE current_validator_insights SET last_good_verdict_received_at = '2025-01-01T00:00:00Z'")
                .execute(state.db().pool()).await.unwrap();
            assert_eq!(
                negative_verdict_public_insight(&state).await["activityState"],
                "stale"
            );
            let recovered = refresh_all(state.db(), &provider).await.unwrap();
            let insight = negative_verdict_public_insight(&state).await;
            assert_eq!(insight["currentValidatorStatus"], "validator");
            assert_eq!(insight["activity"], "active");
            assert_eq!(insight["activityState"], "current");
            assert!(
                !recovered.invalidated_validator_ids.is_empty(),
                "an identical confirmed recovery must refetch stale/error projections"
            );
            let snapshots: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM validator_daily_snapshots")
                    .fetch_one(state.db().pool())
                    .await
                    .unwrap();
            assert_eq!(
                snapshots, 1,
                "replay must not duplicate a historical sample"
            );
        }
    }

    #[tokio::test]
    async fn metric_only_replay_does_not_replace_or_renew_confirmed_absence() {
        let (_dir, state) = negative_verdict_test_state(&provider_node_id()).await;
        let observation = ValidatorObservation {
            provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
            stake_amount: Some("123456789000000000000".to_owned()),
            ..Default::default()
        };
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(observation.clone())),
                ValidatorProviderResult::AuthoritativeEmpty,
                ValidatorProviderResult::Success(Box::new(observation)),
            ]),
            ..Default::default()
        };
        refresh_all(state.db(), &provider).await.unwrap();
        refresh_all(state.db(), &provider).await.unwrap();
        sqlx::query("UPDATE current_validator_insights SET last_good_verdict_received_at = '2025-01-01T00:00:00Z'")
            .execute(state.db().pool()).await.unwrap();
        refresh_all(state.db(), &provider).await.unwrap();
        let insight = negative_verdict_public_insight(&state).await;
        assert_eq!(insight["activityReceivedAt"], "2025-01-01T00:00:00Z");
        assert_eq!(insight["currentValidatorStatus"], "not_validator");
        assert_eq!(insight["activity"], "observing");
        assert_eq!(insight["activityState"], "stale");
        assert_eq!(insight["freshness"], "fresh");
        assert_eq!(insight["stakeAmount"], "123456789000000000000");
    }

    #[tokio::test]
    async fn platscan_rejects_mismatch_malformed_types_and_unsuccessful_responses() {
        let node_id = provider_node_id();
        let other = format!("0x{}", "cd".repeat(64));
        let responses: Vec<(u16, Vec<u8>)> = vec![
            (
                200,
                serde_json::to_vec(&platscan_success(&other, 3)).unwrap(),
            ),
            (
                200,
                serde_json::to_vec(&serde_json::json!({ "code": 0 })).unwrap(),
            ),
            (
                200,
                serde_json::to_vec(&serde_json::json!({
                    "code": 0,
                    "data": { "nodeId": node_id, "status": "3" }
                }))
                .unwrap(),
            ),
            (
                200,
                serde_json::to_vec(&platscan_success(&node_id, 99)).unwrap(),
            ),
            (
                200,
                serde_json::to_vec(&serde_json::json!({
                    "code": 1,
                    "data": { "nodeId": node_id, "status": 3 }
                }))
                .unwrap(),
            ),
            (200, b"not-json".to_vec()),
            (500, Vec::new()),
            (405, Vec::new()),
        ];
        let (base_url, _state, handle) = start_mock_platscan(responses, 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        for _ in 0..7 {
            assert!(matches!(
                provider.fetch("platon-mainnet", &node_id).await,
                ValidatorProviderResult::Error(_)
            ));
        }
        assert!(matches!(
            provider.fetch("platon-mainnet", &node_id).await,
            ValidatorProviderResult::Unsupported(_)
        ));
        handle.abort();
    }

    #[tokio::test]
    async fn platscan_rejects_oversized_bodies_and_times_out() {
        let oversized = vec![b'x'; MAX_PROVIDER_BODY_LEN + 1];
        let (base_url, _state, handle) = start_mock_platscan(vec![(200, oversized)], 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(
            provider.fetch("platon-mainnet", &provider_node_id()).await,
            ValidatorProviderResult::Error(_)
        ));
        handle.abort();

        let (base_url, _state, handle) = start_mock_platscan(Vec::new(), 500).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_millis(100),
        )
        .unwrap();
        assert!(matches!(
            provider.fetch("platon-mainnet", &provider_node_id()).await,
            ValidatorProviderResult::Error(_)
        ));
        handle.abort();
    }

    #[test]
    fn platscan_url_and_network_binding_validation_are_bounded() {
        assert!(normalize_provider_base_url(" https://scan.example.com/ ").is_ok());
        assert_eq!(
            normalize_provider_base_url("https://scan.example.com/base/").unwrap(),
            "https://scan.example.com/base"
        );
        for raw in [
            "ftp://scan.example.com",
            "https://user@scan.example.com",
            "https://user:pass@scan.example.com",
            "https://scan.example.com?query=1",
            "https://scan.example.com#fragment",
            "https://",
            "https:///path",
            "https://host with spaces",
            "https://host:bad",
            "not-a-url",
        ] {
            assert!(normalize_provider_base_url(raw).is_err(), "{raw}");
        }
        let empty = BTreeMap::new();
        assert!(validate_provider_deployments(&empty).is_err());
        let blank = BTreeMap::from([("".to_owned(), "https://scan.example.com".to_owned())]);
        assert!(validate_provider_deployments(&blank).is_err());
        let blank_url = BTreeMap::from([("platon-mainnet".to_owned(), "  ".to_owned())]);
        assert!(validate_provider_deployments(&blank_url).is_err());
        let many = (0..65)
            .map(|index| (format!("n{index}"), "https://scan.example.com".to_owned()))
            .collect::<BTreeMap<_, _>>();
        assert!(validate_provider_deployments(&many).is_err());
    }
    #[tokio::test]
    async fn provider_refresh_preserves_last_good_and_confirms_rank_after_two_observations() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xprovider", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    stake_amount: Some("123456789012345678901234567890".to_owned()),
                    reward_rate: Some("0.125000000000000001".to_owned()),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    stake_amount: Some("123456789012345678901234567889".to_owned()),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:02:00Z".to_owned()),
                    stake_amount: Some("123456789012345678901234567888".to_owned()),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Error(
                    "provider timeout at https://secret.example".to_owned(),
                ),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xprovider", 1)]),
                ranking_with(3, &[("0xprovider", 2)]),
                ranking_with(3, &[("0xprovider", 2)]),
                RankingProviderResult::Error("ranking timeout".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        let first = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(first.attempted, 1);
        assert_eq!(first.invalidations, 1);
        let second = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(second.changed, 0);
        assert_eq!(second.invalidations, 1);
        let third = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(third.changed, 1);
        assert_eq!(third.invalidations, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM validator_ranking_history")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.rank, Some(2));
        assert_eq!(
            insight.stake_amount.as_deref(),
            Some("123456789012345678901234567888")
        );
        assert_eq!(insight.counter_state, "counter_reset");
        let incidents = sqlx::query_as::<_, (String, String, String)>(
            "SELECT rule_key, state, opened_evidence_json FROM alert_incidents WHERE subject_key = ? ORDER BY rule_key",
        )
        .bind(&validator.validator_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            incidents.len(),
            2,
            "ranking and counter signals are independent"
        );
        assert!(incidents.iter().all(|(rule_key, state, _)| state == "open"
            && (rule_key == "validator.ranking_changed" || rule_key == "validator.counter_reset")));
        let counter_evidence = incidents
            .iter()
            .find(|(rule_key, _, _)| rule_key == "validator.counter_reset")
            .map(|(_, _, evidence)| evidence)
            .unwrap();
        assert!(counter_evidence.contains("123456789012345678901234567890"));
        assert!(counter_evidence.contains("123456789012345678901234567889"));
        let ranking_state: (bool, String) = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE rule_key = 'validator.ranking_changed' AND subject_key = ?",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(ranking_state, (false, "known".to_owned()));
        let counter_state: (bool, String) = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE rule_key = 'validator.counter_reset' AND subject_key = ?",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(counter_state, (false, "known".to_owned()));
        refresh_all(&db, &provider).await.unwrap();
        let failed = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.outcome, "error");
        assert_eq!(failed.rank, Some(2));
        assert_eq!(
            failed.stake_amount.as_deref(),
            Some("123456789012345678901234567888")
        );
        let failed_diagnostic = failed.diagnostic.unwrap_or_default();
        assert!(failed_diagnostic.contains("provider timeout"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM alert_incidents WHERE subject_key = ? AND state = 'open'",
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            2
        );
        let unavailable: Vec<(bool, String)> = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE subject_key = ? AND rule_key LIKE 'validator.%' ORDER BY rule_key",
        )
        .bind(&validator.validator_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            unavailable,
            vec![
                (true, "unsupported".to_owned()),
                (true, "unsupported".to_owned())
            ]
        );
    }

    #[tokio::test]
    async fn candidate_activity_persists_as_its_own_canonical_value() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) =
            create_validator(&db, "platon-mainnet", "0xcandidate", None, &owner_id)
                .await
                .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Candidate),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Active),
                    ..Default::default()
                })),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(1, &[("0xcandidate", 1)]),
                ranking_with(1, &[("0xcandidate", 1)]),
            ]),
            ..FakeProvider::default()
        };

        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        // Candidate is stored under its own canonical value, not folded into
        // Active: the persisted turn must pass the activity CHECK constraint.
        assert_eq!(insight.activity.as_deref(), Some("candidate"));

        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.activity.as_deref(), Some("active"));
    }

    #[tokio::test]
    async fn candidate_activity_migration_preserves_existing_last_good_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("server.db");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        // Build the schema immediately before 0060, then seed a last-good
        // Validator insight exactly as the pre-split Server would have stored it.
        migrator_through(59).run(&pool).await.unwrap();
        seed_legacy_network_agent_and_node(&pool).await;
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-keep', 'platon-mainnet', '0xkeep', NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, stake_amount, counter_state, change_state, candidate_observations, updated_at) VALUES ('validator-keep', 'platscan', 'success', '2026-01-01T00:00:00Z', 'active', '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z', '42', 'normal', 'normal', 0, '2026-01-02T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        // Reproduce the runtime failure when a Candidate observation reaches
        // a database that has not applied migration 0060 yet.
        let error = sqlx::query(
            "UPDATE current_validator_insights SET activity = 'candidate' WHERE validator_id = 'validator-keep'",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        let database_error = error.as_database_error().unwrap();
        assert_eq!(database_error.code().as_deref(), Some("275"));
        assert_eq!(
            database_error.message(),
            "CHECK constraint failed: activity IN ('active', 'producing', 'exiting', 'exited', 'verifying', 'locked')"
        );
        pool.close().await;
        #[cfg(unix)]
        restrict_database_permissions(&path);

        let db = initialize(ServerDatabaseConfig::new(&path)).await.unwrap();

        // The table rebuild copies the stored row verbatim: no last-good value
        // is rewritten or erased by the widened CHECK.
        let row: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT outcome, activity, stake_amount FROM current_validator_insights WHERE validator_id = 'validator-keep'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                "success".to_owned(),
                Some("active".to_owned()),
                Some("42".to_owned())
            )
        );
        // The widened CHECK now admits the split Candidate value.
        sqlx::query(
            "UPDATE current_validator_insights SET activity = 'candidate' WHERE validator_id = 'validator-keep'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let activity: Option<String> = sqlx::query_scalar(
            "SELECT activity FROM current_validator_insights WHERE validator_id = 'validator-keep'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(activity.as_deref(), Some("candidate"));
    }

    #[tokio::test]
    async fn activity_persists_applies_immediately_and_survives_provider_failure() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xactivity", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Active),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Producing),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:02:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Producing),
                    ..Default::default()
                })),
                ValidatorProviderResult::Error("provider timeout".to_owned()),
                ValidatorProviderResult::AuthoritativeEmpty,
                // A validated snapshot that omits Activity preserves the
                // last-good canonical value instead of erasing it.
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:03:00Z".to_owned()),
                    stake_amount: Some("1".to_owned()),
                    ..Default::default()
                })),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xactivity", 1)]),
                ranking_with(3, &[("0xactivity", 2)]),
                ranking_with(3, &[("0xactivity", 2)]),
                ranking_with(3, &[("0xactivity", 2)]),
                ranking_with(3, &[("0xactivity", 2)]),
                ranking_with(3, &[("0xactivity", 2)]),
            ]),
            ..FakeProvider::default()
        };

        let first = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(first.invalidations, 1);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.activity.as_deref(), Some("active"));

        // Activity applies on the very next successful snapshot. The upstream
        // rank is adopted immediately, while the ranking-change alert waits
        // for the second consecutive observation to confirm.
        let second = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(second.changed, 1);
        assert_eq!(second.invalidations, 1);
        assert_eq!(
            second.invalidated_validator_ids,
            vec![validator.validator_id.clone()]
        );
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.activity.as_deref(), Some("producing"));
        assert_eq!(insight.rank, Some(2));
        assert_eq!(insight.candidate_rank, Some(2));

        let third = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(third.changed, 1);
        assert_eq!(third.invalidations, 1);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.activity.as_deref(), Some("producing"));
        assert_eq!(insight.rank, Some(2));
        assert_eq!(insight.candidate_rank, None);

        // Provider failure keeps the last-good Activity: Error outcomes
        // retain Producing so the Public projection can mark it Stale.
        let failed = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(failed.invalidations, 1);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.activity.as_deref(), Some("producing"));

        // An authoritative empty result is still Activity-less for the
        // Public projection (Observing) and invalidates the projection.
        let empty = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(empty.invalidations, 1);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "empty");

        assert_eq!(insight.last_good_verdict_outcome.as_deref(), Some("empty"));
        let absence_received_at = insight.last_good_verdict_received_at.clone();

        // Metric-only success retains the raw cache, but cannot supersede or
        // renew the last confirmed absence.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.rank, Some(2));
        assert_eq!(insight.activity.as_deref(), Some("producing"));
        assert_eq!(insight.last_good_verdict_outcome.as_deref(), Some("empty"));
        assert_eq!(insight.last_good_verdict_received_at, absence_received_at);
        let (activity, currency, status) = project_verdict(
            &insight.outcome,
            insight.last_good_verdict_outcome.as_deref(),
            insight.activity.as_deref(),
            "fresh",
        );
        assert_eq!(
            (activity.as_str(), currency.as_str()),
            ("observing", "current")
        );
        assert_eq!(status.status, CurrentValidatorStatus::NotValidator);

        // A Validator that has never seen a successful Activity stays Unknown
        // even after an Error; Provider state never fabricates a value.
        let (unknown, _) = create_validator(&db, "platon-mainnet", "0xunknown", None, &owner_id)
            .await
            .unwrap();
        let failed_first = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Error("never seen".to_owned()),
                ValidatorProviderResult::Error("never seen".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &failed_first).await.unwrap();
        let insight = load_insight(&db, &unknown.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.activity, None);
    }

    #[tokio::test]
    async fn ranking_changes_need_consecutive_successes_and_replay_is_idempotent() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xrank", None, &owner_id)
            .await
            .unwrap();
        fn detail(provider_timestamp: &str) -> ValidatorProviderResult {
            ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                provider_timestamp: Some(provider_timestamp.to_owned()),
                stake_amount: Some("1000".to_owned()),
                ..ValidatorObservation::default()
            }))
        }
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                detail("2025-01-01T00:00:00Z"),
                detail("2025-01-01T00:01:00Z"),
                detail("2025-01-01T00:02:00Z"),
                ValidatorProviderResult::Error("temporary detail failure".to_owned()),
                detail("2025-01-01T00:03:00Z"),
                detail("2025-01-01T00:04:00Z"),
                detail("2025-01-01T00:05:00Z"),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xrank", 1)]),
                ranking_with(3, &[("0xrank", 2)]),
                ranking_with(3, &[("0xrank", 2)]),
                RankingProviderResult::Error("temporary ranking failure".to_owned()),
                ranking_with(3, &[("0xrank", 3)]),
                ranking_with(3, &[("0xrank", 3)]),
                ranking_with(3, &[("0xrank", 3)]),
            ]),
            ..FakeProvider::default()
        };

        async fn history_count(db: &ServerDatabase, validator_id: &str) -> i64 {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_ranking_history WHERE validator_id = ?",
            )
            .bind(validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap()
        }

        // One success establishes the baseline and is never a change.
        refresh_all(&db, &provider).await.unwrap();
        let baseline = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(baseline.rank, Some(1));
        assert_eq!(baseline.rank_outcome.as_deref(), Some("success"));
        assert_eq!(history_count(&db, &validator.validator_id).await, 0);

        // A second distinct rank is a candidate, not yet a confirmed change.
        refresh_all(&db, &provider).await.unwrap();
        let candidate = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.rank, Some(2));
        assert_eq!(candidate.candidate_rank, Some(2));
        assert_eq!(history_count(&db, &validator.validator_id).await, 0);
        db.close().await;
        let db = initialize(ServerDatabaseConfig::new(_dir.path().join("server.db")))
            .await
            .unwrap();

        // A consecutive successful list confirms the change across restart.
        refresh_all(&db, &provider).await.unwrap();
        let confirmed = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.rank, Some(2));
        assert_eq!(confirmed.candidate_rank, None);
        assert_eq!(history_count(&db, &validator.validator_id).await, 1);

        // A failed list fetch retains the rank and discards the candidate.
        refresh_all(&db, &provider).await.unwrap();
        let after_failure = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_failure.rank, Some(2));
        assert_eq!(after_failure.rank_outcome.as_deref(), Some("error"));
        assert_eq!(after_failure.candidate_rank, None);
        assert_eq!(history_count(&db, &validator.validator_id).await, 1);

        // After a failure the next success is a fresh candidate, never a
        // confirmation stitched across the gap.
        refresh_all(&db, &provider).await.unwrap();
        let rearmed = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rearmed.rank, Some(3));
        assert_eq!(rearmed.candidate_rank, Some(3));
        assert_eq!(history_count(&db, &validator.validator_id).await, 1);

        refresh_all(&db, &provider).await.unwrap();
        let confirmed_again = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(confirmed_again.rank, Some(3));
        assert_eq!(history_count(&db, &validator.validator_id).await, 2);

        // Replaying the same confirmed list is idempotent.
        refresh_all(&db, &provider).await.unwrap();
        assert_eq!(history_count(&db, &validator.validator_id).await, 2);
    }

    #[tokio::test]
    async fn counter_correction_history_keeps_exact_decimal_evidence() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xcounter", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("100.000000000000000001".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("99.999999999999999999".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("99.999999999999999999".to_owned()),
                    ..Default::default()
                })),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();
        let row = sqlx::query_as::<_, ValidatorCounterHistoryRecord>(
            "SELECT history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key FROM validator_counter_history WHERE validator_id = ?",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(row.counter_name, "stake_amount");
        assert_eq!(row.previous_value, "100.000000000000000001");
        assert_eq!(row.current_value, "99.999999999999999999");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_counter_history WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn reward_amount_decrease_is_recorded_as_a_counter_correction() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xreward", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    reward_amount: Some("500.000000000012".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    reward_amount: Some("499.999999999999".to_owned()),
                    ..Default::default()
                })),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();

        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        // The exact 12-decimal source value is retained and the falling
        // cumulative reward is flagged as a reset/correction rather than
        // silently rewritten as normal growth.
        assert_eq!(insight.reward_amount.as_deref(), Some("499.999999999999"));
        assert_eq!(insight.counter_state, "counter_reset");
        let row = sqlx::query_as::<_, ValidatorCounterHistoryRecord>(
            "SELECT history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key FROM validator_counter_history WHERE validator_id = ?",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(row.counter_name, "reward_amount");
        assert_eq!(row.previous_value, "500.000000000012");
        assert_eq!(row.current_value, "499.999999999999");
    }

    #[tokio::test]
    async fn ending_a_link_preserves_history_and_allows_replacement() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xdef", None, &owner_id)
            .await
            .unwrap();
        let (link, _) = create_link(
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
        end_link(&db, &link.link_id, Some("2025-03-01T00:00:00Z"), &owner_id)
            .await
            .unwrap();
        assert!(matches!(
            end_link(&db, &link.link_id, Some("2025-03-02T00:00:00Z"), &owner_id).await,
            Err(ValidatorError::LinkAlreadyEnded)
        ));

        let (replacement, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "standby",
            "2025-03-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        assert_eq!(replacement.role.as_deref(), Some("standby"));
        assert_eq!(
            list_links(&db, Some("node-1"), None, None)
                .await
                .unwrap()
                .len(),
            2
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
                "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );

        // A Server restart must not replay or duplicate the accepted sample.
        db.close().await;
        let db = initialize(ServerDatabaseConfig::new(_dir.path().join("server.db")))
            .await
            .unwrap();
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "America/Los_Angeles")
            .await
            .unwrap();
        let daily_after_restart = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily_after_restart.len(), 1);
        let monthly_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = ?",
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
                "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }

    #[test]
    fn freshness_window_defaults_to_two_refresh_intervals() {
        let now = crate::auth::now_utc();
        let fresh_at = crate::auth::format_rfc3339(now - time::Duration::seconds(119));
        let stale_at = crate::auth::format_rfc3339(now - time::Duration::seconds(121));
        assert_eq!(freshness(Some(&fresh_at), now, 120), "fresh");
        assert_eq!(freshness(Some(&stale_at), now, 120), "stale");
        // A slower configured refresh interval keeps the same age fresh.
        assert_eq!(freshness(Some(&stale_at), now, 600), "fresh");
        assert_eq!(freshness(None, now, 600), "unknown");
    }

    #[test]
    fn decimal_accumulation_is_exact_and_rejects_malformed_values() {
        // Large integer and fractional magnitudes keep every source digit; a
        // binary float would already have lost them.
        let mut total: Option<String> = None;
        assert!(accumulate_decimal(
            &mut total,
            "111111111111111111111.111111111111"
        ));
        assert!(accumulate_decimal(
            &mut total,
            "222222222222222222222.222222222222"
        ));
        assert_eq!(total.as_deref(), Some("333333333333333333333.333333333333"));
        // Mixed precision pads to the widest source scale and preserves
        // trailing fractional zeros.
        let mut mixed: Option<String> = None;
        assert!(accumulate_decimal(&mut mixed, "10"));
        assert!(accumulate_decimal(&mut mixed, "0.10"));
        assert!(accumulate_decimal(&mut mixed, "2.2"));
        assert_eq!(mixed.as_deref(), Some("12.30"));
        // Leading-dot and trailing-dot syntax normalizes without inventing
        // precision beyond the source.
        let mut dotted: Option<String> = None;
        assert!(accumulate_decimal(&mut dotted, ".5"));
        assert!(accumulate_decimal(&mut dotted, ".5"));
        assert_eq!(dotted.as_deref(), Some("1.0"));
        let mut trailing_dot: Option<String> = None;
        assert!(accumulate_decimal(&mut trailing_dot, "12."));
        assert!(accumulate_decimal(&mut trailing_dot, "1"));
        assert_eq!(trailing_dot.as_deref(), Some("13"));
        // A valid zero stays a real value, distinct from "no value at all".
        let mut zero: Option<String> = None;
        assert!(accumulate_decimal(&mut zero, "0"));
        assert!(accumulate_decimal(&mut zero, "0.000"));
        assert_eq!(zero.as_deref(), Some("0.000"));
        // Malformed values are rejected without changing the running total.
        let mut rejected: Option<String> = None;
        assert!(accumulate_decimal(&mut rejected, "7"));
        for malformed in ["", "abc", "1e3", "-1", "1.2.3", "0x10", " 1"] {
            assert!(
                !accumulate_decimal(&mut rejected, malformed),
                "{malformed:?} must be rejected"
            );
        }
        assert_eq!(rejected.as_deref(), Some("7"));
        // An all-zero run normalizes the integer part instead of leaving
        // unbounded leading zeros.
        let mut leading: Option<String> = None;
        assert!(accumulate_decimal(&mut leading, "000.5"));
        assert!(accumulate_decimal(&mut leading, "000.5"));
        assert_eq!(leading.as_deref(), Some("1.0"));
    }

    #[test]
    fn decimal_accumulation_grows_past_the_source_bound_without_panicking() {
        // Each source value is within the 256-character trust-boundary bound,
        // but the exact running total legitimately grows one digit past it.
        // Re-reading that total must not re-apply the source bound and panic.
        let huge = "9".repeat(256);
        let mut total: Option<String> = None;
        for _ in 0..3 {
            assert!(accumulate_decimal(&mut total, &huge));
        }
        let expected = format!("2{}7", "9".repeat(255));
        assert_eq!(total.as_deref(), Some(expected.as_str()));
        assert_eq!(expected.len(), 257);
    }

    #[tokio::test]
    async fn not_configured_network_is_explicit_and_retains_last_good_block_count() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Producing),
                    block_count: Some(100),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::NotConfigured("no bound deployment".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let first = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.outcome, "success");
        assert_eq!(first.block_count, Some(100));
        assert_eq!(first.activity.as_deref(), Some("producing"));

        let summary = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(summary.attempted, 1);
        assert_eq!(summary.successful, 0);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "not_configured");
        assert_eq!(
            insight.block_count,
            Some(100),
            "last-good count is retained"
        );
        assert_eq!(insight.activity.as_deref(), Some("producing"));
        assert_eq!(insight.last_good_received_at, first.last_good_received_at);
        assert_eq!(insight.provider_timestamp, first.provider_timestamp);
        assert_eq!(insight.counter_state, "normal");
    }

    #[tokio::test]
    async fn migrated_schema_preserves_existing_last_good_insight_row() {
        // Migration 0044 widens the outcome CHECK to allow not_configured. A
        // pre-existing unsupported row keeps its retained last-good values.
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, last_attempt_received_at, last_good_received_at, block_count, counter_state, change_state, candidate_observations, updated_at) VALUES (?, 'platscan', 'unsupported', 'legacy', '2025-01-01T00:00:00Z', ?, ?, 77, 'normal', 'normal', 0, ?)")
            .bind(&validator.validator_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(db.pool())
            .await
            .unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "unsupported");
        assert_eq!(insight.block_count, Some(77));
        assert_eq!(insight.last_good_received_at.as_deref(), Some(now.as_str()));
        assert_eq!(
            insight.provider_timestamp.as_deref(),
            Some("2025-01-01T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn migration_0046_adds_the_delegation_percentage_without_backfilling_legacy_rows() {
        use sqlx::sqlite::SqlitePoolOptions;

        // Build the schema exactly as it existed immediately before migration
        // 0046, seed one last-good row, then apply 0046 directly. The full
        // startup path also runs the destructive #174 cutover (0053), which
        // removes an unlinked legacy Validator, so scoping this test to 0046
        // keeps it about 0046.
        let dir = tempdir().unwrap();
        let path = dir.path().join("server.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        migrator_through(45).run(&pool).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('platon-mainnet', 'Mainnet', '0x0', 1, 1, 'lat', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-legacy', 'platon-mainnet', '0xabc', NULL, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, last_attempt_received_at, last_good_received_at, block_count, expected_block_count, gen_blocks_rate, updated_at) VALUES ('validator-legacy', 'platscan', 'success', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 100, 110, '75.5', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        migrator_through(46).run(&pool).await.unwrap();
        // Schema 46 predates several columns of the current INSIGHT_SELECT, so
        // this test reads only the columns that exist at that version.
        #[derive(sqlx::FromRow)]
        struct LegacyInsightRow {
            block_count: Option<i64>,
            expected_block_count: Option<i64>,
            gen_blocks_rate: Option<String>,
            delegation_reward_percentage: Option<String>,
        }
        let insight = sqlx::query_as::<_, LegacyInsightRow>(
            "SELECT block_count, expected_block_count, gen_blocks_rate, delegation_reward_percentage FROM current_validator_insights WHERE validator_id = 'validator-legacy'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // The upgrade keeps every existing last-good value...
        assert_eq!(insight.block_count, Some(100));
        assert_eq!(insight.expected_block_count, Some(110));
        assert_eq!(insight.gen_blocks_rate.as_deref(), Some("75.5"));
        // ... and the new column stays Unknown instead of acquiring a value.
        assert_eq!(insight.delegation_reward_percentage, None);
        pool.close().await;
    }

    #[tokio::test]
    async fn block_count_survives_database_reopen_without_fabricating_values() {
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    activity: Some(ValidatorActivity::Active),
                    block_count: Some(55),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    activity: Some(ValidatorActivity::Active),
                    ..ValidatorObservation::default()
                })),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let validator_id = validator.validator_id.clone();
        drop(db);

        let reopened = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.block_count, Some(55));

        // A successful observation that omits blockQty is Unknown, never a
        // fabricated zero.
        refresh_all(&reopened, &provider).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.block_count, None);
    }

    #[test]
    fn cumulative_block_rate_is_exact_and_never_mixes_observations() {
        assert_eq!(
            cumulative_block_rate(Some(100), Some(110)),
            (Some("90.909091".to_owned()), "ok")
        );
        assert_eq!(
            cumulative_block_rate(Some(1), Some(2)),
            (Some("50".to_owned()), "ok")
        );
        assert_eq!(
            cumulative_block_rate(Some(2), Some(3)),
            (Some("66.666667".to_owned()), "ok")
        );
        assert_eq!(
            cumulative_block_rate(Some(0), Some(5)),
            (Some("0".to_owned()), "ok")
        );
        // A known zero denominator is not applicable, never 0% or 100%,
        // whether or not the numerator is known.
        assert_eq!(
            cumulative_block_rate(Some(0), Some(0)),
            (None, "not_applicable")
        );
        assert_eq!(
            cumulative_block_rate(None, Some(0)),
            (None, "not_applicable")
        );
        // An incomplete pair stays unknown rather than mixing observations.
        assert_eq!(cumulative_block_rate(Some(200), None), (None, "unknown"));
        assert_eq!(cumulative_block_rate(None, Some(50)), (None, "unknown"));
        assert_eq!(cumulative_block_rate(None, None), (None, "unknown"));
    }

    #[test]
    fn platscan_normalizes_both_rate_inputs_and_rejects_invalid_percentages() {
        let node_id = provider_node_id();
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "nodeId": node_id,
                "status": 3,
                "blockQty": 100,
                "expectBlockQty": 110,
                "genBlocksRate": "90.909091%"
            }
        });
        let observation = normalize_platscan_response(&body, &node_id)
            .unwrap()
            .unwrap();
        assert_eq!(observation.expected_block_count, Some(110));
        assert_eq!(observation.gen_blocks_rate.as_deref(), Some("90.909091"));

        // A source-reported zero percentage is retained as a source value,
        // distinct from a genuinely missing field.
        let zero = serde_json::json!({
            "code": 0,
            "data": { "nodeId": node_id, "status": 3, "genBlocksRate": "0%" }
        });
        assert_eq!(
            normalize_platscan_response(&zero, &node_id)
                .unwrap()
                .unwrap()
                .gen_blocks_rate
                .as_deref(),
            Some("0")
        );
        let missing = serde_json::json!({
            "code": 0,
            "data": { "nodeId": node_id, "status": 3 }
        });
        assert_eq!(
            normalize_platscan_response(&missing, &node_id)
                .unwrap()
                .unwrap()
                .gen_blocks_rate,
            None
        );

        // A JSON number is a legal rate, including a fractional one; a rate is
        // not an exact monetary amount, so it is not held to the amount rule.
        let numeric = serde_json::json!({
            "code": 0,
            "data": { "nodeId": node_id, "status": 3, "genBlocksRate": 12.5 }
        });
        assert_eq!(
            normalize_platscan_response(&numeric, &node_id)
                .unwrap()
                .unwrap()
                .gen_blocks_rate
                .as_deref(),
            Some("12.5")
        );

        // Malformed, stray-percent, or negative values degrade the whole
        // observation to Error instead of synthesizing 0%.
        for invalid in [
            serde_json::json!({ "code": 0, "data": { "nodeId": node_id, "status": 3, "genBlocksRate": "12%%" } }),
            serde_json::json!({ "code": 0, "data": { "nodeId": node_id, "status": 3, "genBlocksRate": "%12" } }),
            serde_json::json!({ "code": 0, "data": { "nodeId": node_id, "status": 3, "genBlocksRate": "abc" } }),
            serde_json::json!({ "code": 0, "data": { "nodeId": node_id, "status": 3, "genBlocksRate": -1 } }),
            serde_json::json!({ "code": 0, "data": { "nodeId": node_id, "status": 3, "expectBlockQty": -1 } }),
        ] {
            assert!(
                normalize_platscan_response(&invalid, &node_id).is_err(),
                "invalid payload {invalid}"
            );
        }
    }

    #[tokio::test]
    async fn block_rates_persist_retain_last_good_and_never_splice_observations() {
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let validator_id = validator.validator_id.clone();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Producing),
                    block_count: Some(100),
                    expected_block_count: Some(110),
                    gen_blocks_rate: Some("90.909091".to_owned()),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator_id).await.unwrap().unwrap();
        assert_eq!(insight.block_count, Some(100));
        assert_eq!(insight.expected_block_count, Some(110));
        assert_eq!(insight.gen_blocks_rate.as_deref(), Some("90.909091"));

        // Restart persistence: the rate rows survive a database reopen.
        drop(db);
        let reopened = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.expected_block_count, Some(110));
        assert_eq!(insight.gen_blocks_rate.as_deref(), Some("90.909091"));

        // A non-success outcome retains every last-good rate value.
        let failure = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Error(
                "platscan failed".to_owned(),
            )]),
            ..FakeProvider::default()
        };
        refresh_all(&reopened, &failure).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.expected_block_count, Some(110));
        assert_eq!(insight.gen_blocks_rate.as_deref(), Some("90.909091"));
        assert_eq!(
            cumulative_block_rate(insight.block_count, insight.expected_block_count),
            (Some("90.909091".to_owned()), "ok")
        );

        // A later success with a fresh numerator but no denominator clears the
        // pair: the new numerator is never divided by the older denominator.
        let partial = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    activity: Some(ValidatorActivity::Producing),
                    block_count: Some(200),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&reopened, &partial).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.block_count, Some(200));
        assert_eq!(insight.expected_block_count, None);
        assert_eq!(insight.gen_blocks_rate, None);
        assert_eq!(
            cumulative_block_rate(insight.block_count, insight.expected_block_count),
            (None, "unknown")
        );
    }

    #[tokio::test]
    async fn delegation_reward_percentage_persists_retains_last_good_and_survives_restart() {
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let validator_id = validator.validator_id.clone();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    activity: Some(ValidatorActivity::Producing),
                    delegation_reward_percentage: Some("20".to_owned()),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator_id).await.unwrap().unwrap();
        assert_eq!(insight.delegation_reward_percentage.as_deref(), Some("20"));

        // The percentage survives a Server restart (database reopen).
        drop(db);
        let reopened = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.delegation_reward_percentage.as_deref(), Some("20"));

        // A non-success outcome retains the last-good percentage.
        let failure = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Error(
                "platscan failed".to_owned(),
            )]),
            ..FakeProvider::default()
        };
        refresh_all(&reopened, &failure).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.delegation_reward_percentage.as_deref(), Some("20"));

        // A later success that omits rewardPer is Unknown, never carried from a
        // different observation and never fabricated as zero.
        let partial = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    activity: Some(ValidatorActivity::Producing),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&reopened, &partial).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.delegation_reward_percentage, None);
    }
    fn ranking_node_id(index: usize) -> String {
        format!("0x{index:0128x}")
    }

    #[tokio::test]
    async fn platscan_ranking_adapter_uses_the_all_cohort_without_a_name_filter() {
        let node_a = ranking_node_id(1);
        let node_b = ranking_node_id(2);
        let body = serde_json::to_vec(&platscan_ranking_response(
            1,
            RANKING_PAGE_SIZE,
            2,
            &[node_a.clone(), node_b.clone()],
        ))
        .unwrap();
        let (base_url, state, handle) = start_mock_platscan(vec![(200, body)], 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        match provider.fetch_ranking("platon-mainnet").await {
            RankingProviderResult::Success(ranking) => {
                assert_eq!(ranking.cohort_size, 2);
                assert_eq!(ranking.entries.get(&node_a), Some(&1));
                assert_eq!(ranking.entries.get(&node_b), Some(&2));
            }
            other => panic!("expected a successful ranking, got {other:?}"),
        }
        handle.abort();
        let requests = state.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "a single complete page is one request");
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/browser-server/staking/aliveStakingList");
        let request_body: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(request_body["queryStatus"], "all");
        assert_eq!(request_body["pageNo"], 1);
        assert_eq!(request_body["pageSize"], RANKING_PAGE_SIZE);
        assert!(
            request_body.get("key").is_none(),
            "the ALL cohort must never add a name filter"
        );
    }

    #[tokio::test]
    async fn platscan_ranking_adapter_pages_across_the_cohort_and_keeps_global_ranks() {
        let first: Vec<String> = (1..=RANKING_PAGE_SIZE).map(ranking_node_id).collect();
        let target = ranking_node_id(RANKING_PAGE_SIZE + 1);
        let total = RANKING_PAGE_SIZE as i64 + 1;
        let page_one = serde_json::to_vec(&platscan_ranking_response(
            1,
            RANKING_PAGE_SIZE,
            total,
            &first,
        ))
        .unwrap();
        let page_two = serde_json::to_vec(&platscan_ranking_response(
            2,
            RANKING_PAGE_SIZE,
            total,
            std::slice::from_ref(&target),
        ))
        .unwrap();
        let (base_url, state, handle) =
            start_mock_platscan(vec![(200, page_one), (200, page_two)], 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        match provider.fetch_ranking("platon-mainnet").await {
            RankingProviderResult::Success(ranking) => {
                assert_eq!(ranking.cohort_size, total);
                assert_eq!(ranking.entries.len(), RANKING_PAGE_SIZE + 1);
                assert_eq!(ranking.entries.get(&target), Some(&total));
            }
            other => panic!("expected a successful ranking, got {other:?}"),
        }
        handle.abort();
        let requests = state.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        let second_page: Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(second_page["pageNo"], 2);
    }

    async fn ranking_fetch(responses: Vec<(u16, Vec<u8>)>) -> RankingProviderResult {
        let (base_url, _state, handle) = start_mock_platscan(responses, 0).await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        let result = provider.fetch_ranking("platon-mainnet").await;
        handle.abort();
        result
    }

    #[tokio::test]
    async fn platscan_ranking_adapter_rejects_incomplete_drifted_and_duplicate_pages() {
        let first: Vec<String> = (1..=RANKING_PAGE_SIZE).map(ranking_node_id).collect();
        let target = ranking_node_id(RANKING_PAGE_SIZE + 1);
        let total = RANKING_PAGE_SIZE as i64 + 1;
        // A short page that never reaches the declared total is incomplete.
        let short_page = serde_json::to_vec(&platscan_ranking_response(
            1,
            RANKING_PAGE_SIZE,
            total,
            &[ranking_node_id(1)],
        ))
        .unwrap();
        assert!(matches!(
            ranking_fetch(vec![(200, short_page)]).await,
            RankingProviderResult::Error(_)
        ));
        // A page whose declared total drifts from the first page is invalid.
        let page_one = serde_json::to_vec(&platscan_ranking_response(
            1,
            RANKING_PAGE_SIZE,
            total,
            &first,
        ))
        .unwrap();
        let drift_page = serde_json::to_vec(&platscan_ranking_response(
            2,
            RANKING_PAGE_SIZE,
            total + 1,
            std::slice::from_ref(&target),
        ))
        .unwrap();
        assert!(matches!(
            ranking_fetch(vec![(200, page_one.clone()), (200, drift_page)]).await,
            RankingProviderResult::Error(_)
        ));
        // A Validator that appears on two pages cannot be counted twice.
        let duplicate_page = serde_json::to_vec(&platscan_ranking_response(
            2,
            RANKING_PAGE_SIZE,
            total,
            &[first[RANKING_PAGE_SIZE - 1].clone()],
        ))
        .unwrap();
        assert!(matches!(
            ranking_fetch(vec![(200, page_one.clone()), (200, duplicate_page)]).await,
            RankingProviderResult::Error(_)
        ));
        // A page-local rank sequence is a detectable inconsistency, not a rank.
        let local_rank = serde_json::to_vec(&serde_json::json!({
            "code": 0,
            "errMsg": "success",
            "totalCount": total,
            "data": [{ "nodeId": target, "ranking": 1 }],
        }))
        .unwrap();
        assert!(matches!(
            ranking_fetch(vec![(200, page_one), (200, local_rank)]).await,
            RankingProviderResult::Error(_)
        ));
        // A cohort beyond the bound is refused before it can grow the fetch.
        let oversized = serde_json::to_vec(&serde_json::json!({
            "code": 0,
            "totalCount": MAX_RANKING_COHORT + 1,
            "data": [],
        }))
        .unwrap();
        assert!(matches!(
            ranking_fetch(vec![(200, oversized)]).await,
            RankingProviderResult::Error(_)
        ));
        // A transport failure and an unsupported endpoint are distinct.
        assert!(matches!(
            ranking_fetch(vec![(500, Vec::new())]).await,
            RankingProviderResult::Error(_)
        ));
        assert!(matches!(
            ranking_fetch(vec![(501, Vec::new())]).await,
            RankingProviderResult::Unsupported(_)
        ));
        assert!(matches!(
            ranking_fetch(vec![(404, Vec::new())]).await,
            RankingProviderResult::Unsupported(_)
        ));
    }

    #[tokio::test]
    async fn platscan_ranking_routes_each_network_to_its_own_deployment() {
        let node_a = ranking_node_id(1);
        let node_b = ranking_node_id(2);
        let body_a = serde_json::to_vec(&platscan_ranking_response(
            1,
            RANKING_PAGE_SIZE,
            1,
            std::slice::from_ref(&node_a),
        ))
        .unwrap();
        let body_b = serde_json::to_vec(&platscan_ranking_response(
            1,
            RANKING_PAGE_SIZE,
            1,
            std::slice::from_ref(&node_b),
        ))
        .unwrap();
        let (base_a, state_a, handle_a) = start_mock_platscan(vec![(200, body_a)], 0).await;
        let (base_b, state_b, handle_b) = start_mock_platscan(vec![(200, body_b)], 0).await;
        let mut network_deployments = BTreeMap::new();
        network_deployments.insert("net-a".to_owned(), base_a);
        network_deployments.insert("net-b".to_owned(), base_b);
        let provider =
            PlatScanValidatorProvider::new(network_deployments, std::time::Duration::from_secs(5))
                .unwrap();
        match provider.fetch_ranking("net-a").await {
            RankingProviderResult::Success(ranking) => {
                assert_eq!(ranking.entries.get(&node_a), Some(&1));
                assert_eq!(ranking.entries.get(&node_b), None);
            }
            other => panic!("expected a successful ranking, got {other:?}"),
        }
        match provider.fetch_ranking("net-b").await {
            RankingProviderResult::Success(ranking) => {
                assert_eq!(ranking.entries.get(&node_b), Some(&1));
                assert_eq!(ranking.entries.get(&node_a), None);
            }
            other => panic!("expected a successful ranking, got {other:?}"),
        }
        handle_a.abort();
        handle_b.abort();
        assert_eq!(state_a.requests.lock().unwrap().len(), 1);
        assert_eq!(state_b.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ranking_is_fetched_once_per_network_not_per_validator() {
        let (_dir, db) = test_db().await;
        create_network(
            &db,
            "net-a",
            "Network A",
            "0x00000000000000000000000000000000000000000000000000000000000000a1",
            11,
            11,
            "lat",
        )
        .await
        .unwrap();
        create_network(
            &db,
            "net-b",
            "Network B",
            "0x00000000000000000000000000000000000000000000000000000000000000b1",
            12,
            12,
            "lat",
        )
        .await
        .unwrap();
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        for node_id in ["0xa", "0xb"] {
            create_validator(&db, "net-a", node_id, None, &owner_id)
                .await
                .unwrap();
        }
        create_validator(&db, "net-b", "0xc", None, &owner_id)
            .await
            .unwrap();
        let detail = || {
            ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                stake_amount: Some("1".to_owned()),
                ..ValidatorObservation::default()
            }))
        };
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![detail(), detail(), detail()]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xa", 1), ("0xb", 1), ("0xc", 1)]),
                ranking_with(3, &[("0xa", 1), ("0xb", 1), ("0xc", 1)]),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let calls = provider.ranking_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "one ranking fetch per distinct Network");
        assert!(calls.iter().any(|key| key == "net-a"));
        assert!(calls.iter().any(|key| key == "net-b"));
        assert_eq!(provider.calls.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn only_a_complete_list_establishes_unranked_and_failures_retain_last_good() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabsent", None, &owner_id)
            .await
            .unwrap();
        let detail = || {
            ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                stake_amount: Some("11".to_owned()),
                ..ValidatorObservation::default()
            }))
        };
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![detail(), detail(), detail()]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(2, &[("0xother", 1)]),
                RankingProviderResult::Error("incomplete page".to_owned()),
                ranking_with(2, &[("0xabsent", 1)]),
            ]),
            ..FakeProvider::default()
        };
        // A complete list that omits the Validator is an authoritative unranked.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.rank_outcome.as_deref(), Some("success"));
        assert_eq!(insight.rank, None);
        // An incomplete or failed list must never be mistaken for unranked.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.rank_outcome.as_deref(), Some("error"));
        assert_eq!(insight.rank, None);
        assert_eq!(insight.stake_amount.as_deref(), Some("11"));
        // A later complete list that contains the Validator sets the rank.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.rank_outcome.as_deref(), Some("success"));
        assert_eq!(insight.rank, Some(1));
        assert_eq!(insight.rank_cohort_size, Some(2));
    }

    #[tokio::test]
    async fn ranking_failure_retains_a_last_good_rank_across_detail_failure() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xkeep", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("7".to_owned()),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Error("detail down".to_owned()),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(5, &[("0xkeep", 4)]),
                RankingProviderResult::Error("ranking down".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        // The detail failure retains the ranked value even though the ranking
        // list also failed: the two sources stay independent.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.rank_outcome.as_deref(), Some("error"));
        assert_eq!(insight.rank, Some(4));
        assert_eq!(insight.stake_amount.as_deref(), Some("7"));
        // A detail success that omits a rank update keeps the ranking last-good.
        let partial = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    stake_amount: Some("8".to_owned()),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &partial).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.stake_amount.as_deref(), Some("8"));
        assert_eq!(insight.rank, Some(4));
    }
    #[tokio::test]
    async fn rank_is_adopted_from_the_network_cohort_not_recomputed_over_monitored_nodes() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xtarget", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    stake_amount: Some("1".to_owned()),
                    ..ValidatorObservation::default()
                },
            ))]),
            // The cohort contains unmonitored Validators, so the position of
            // the one monitored Validator is the upstream ALL-cohort rank.
            rankings: std::sync::Mutex::new(vec![ranking_with(
                10,
                &[
                    ("0xother-1", 1),
                    ("0xother-2", 2),
                    ("0xother-3", 3),
                    ("0xtarget", 4),
                ],
            )]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.rank_outcome.as_deref(), Some("success"));
        assert_eq!(insight.rank, Some(4));
        assert_eq!(insight.rank_cohort_size, Some(10));
    }

    #[tokio::test]
    async fn migration_0047_clears_the_untrusted_legacy_detail_rank() {
        use sqlx::sqlite::SqlitePoolOptions;

        // Build the schema immediately before 0047, seed the old detail-alias
        // rank, then apply 0047 directly. The full startup path also runs the
        // destructive #174 cutover (0053), which removes an unlinked legacy
        // Validator, so scoping this test to 0047 keeps it about 0047.
        let dir = tempdir().unwrap();
        let path = dir.path().join("server.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        migrator_through(46).run(&pool).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('platon-mainnet', 'Mainnet', '0x0', 1, 1, 'lat', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-legacy-rank', 'platon-mainnet', '0xabc', NULL, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, last_attempt_received_at, last_good_received_at, rank, block_count, updated_at) VALUES ('validator-legacy-rank', 'platscan', 'success', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 5, 100, '2025-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        migrator_through(47).run(&pool).await.unwrap();
        // Inspect the pinned historical schema, not the current row shape.
        let (rank, rank_outcome, block_count): (Option<i64>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT rank, rank_outcome, block_count FROM current_validator_insights WHERE validator_id = ?")
                .bind("validator-legacy-rank").fetch_one(&pool).await.unwrap();
        // The detail alias was never an authoritative ranking source, so the
        // upgrade drops it rather than exposing an untrusted position.
        assert_eq!(rank, None);
        assert_eq!(rank_outcome, None);
        assert_eq!(block_count, Some(100));
        pool.close().await;
    }

    fn full_node_key(byte: u8) -> String {
        format!("0x{}", format!("{byte:02x}").repeat(64))
    }

    fn enode_uri(key: &str) -> String {
        format!("enode://{}@10.0.0.1:30303", &key[2..])
    }

    const REGISTERED_GENESIS: &str =
        "0x0000000000000000000000000000000000000000000000000000000000000001";

    async fn set_chain_observation(
        db: &ServerDatabase,
        node_id: &str,
        genesis: &str,
        chain_id: i64,
        p2p: i64,
        hrp: &str,
        enode: Option<&str>,
    ) {
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, enode, updated_at) VALUES (?, ?, ?, ?, ?, ?, '2026-01-01T00:00:00Z') ON CONFLICT(node_id) DO UPDATE SET network_genesis_hash=excluded.network_genesis_hash, network_chain_id=excluded.network_chain_id, network_p2p_network_id=excluded.network_p2p_network_id, network_address_hrp=excluded.network_address_hrp, enode=excluded.enode, updated_at=excluded.updated_at")
            .bind(node_id)
            .bind(genesis)
            .bind(chain_id)
            .bind(p2p)
            .bind(hrp)
            .bind(enode)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn identity_state(db: &ServerDatabase, node_id: &str) -> String {
        sqlx::query_scalar("SELECT state FROM node_validator_identity_status WHERE node_id = ?")
            .bind(node_id)
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    #[test]
    fn project_activity_follows_the_evidence_predicates() {
        // Authoritative absence is Observing, never Unknown and never a
        // fabricated Activity.
        assert_eq!(
            project_activity("empty", None, "fresh"),
            ("observing".to_owned(), "current".to_owned())
        );
        // A 404 is a routing or deployment anomaly, never an observing
        // Validator (#168).
        assert_eq!(
            project_activity("not_found", Some("active"), "fresh"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
        assert_eq!(
            project_activity("success", Some("producing"), "fresh"),
            ("producing".to_owned(), "current".to_owned())
        );
        assert_eq!(
            project_activity("success", Some("producing"), "stale"),
            ("producing".to_owned(), "stale".to_owned())
        );
        // A success without a canonical Activity stays Unknown rather than
        // inheriting an older label.
        assert_eq!(
            project_activity("success", None, "fresh"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
        // A failed refresh retains the last-good Activity, marked Stale.
        assert_eq!(
            project_activity("error", Some("active"), "fresh"),
            ("active".to_owned(), "stale".to_owned())
        );
        assert_eq!(
            project_activity("error", None, "fresh"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
        // Unsupported or unconfigured coverage never projects a stale label.
        assert_eq!(
            project_activity("unsupported", Some("active"), "stale"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
        assert_eq!(
            project_activity("not_configured", None, "unknown"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
    }

    #[test]
    fn automatic_identity_reason_explains_every_unresolved_state() {
        // An identified Node, or a caller that does not project the discovery
        // dimension, carries no reason.
        assert!(automatic_identity_reason(Some("identified")).is_none());
        assert!(automatic_identity_reason(None).is_none());
        for state in [
            "not_evaluated",
            "missing_public_key",
            "invalid_public_key",
            "network_identity_missing",
            "network_identity_mismatch",
            "something_unknown",
        ] {
            let reason = automatic_identity_reason(Some(state))
                .unwrap_or_else(|| panic!("{state} must carry a reason"));
            assert!(!reason.is_empty());
            // The copy never claims ownership, consensus membership, or a
            // negative verdict.
            assert!(!reason.to_lowercase().contains("owner"));
            assert!(!reason.to_lowercase().contains("not a validator"));
        }
    }

    #[test]
    fn last_good_age_is_absent_without_a_last_good_value() {
        let now = OffsetDateTime::parse(
            "2026-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert_eq!(last_good_age_seconds(None, now), None);
        assert_eq!(last_good_age_seconds(Some("not a timestamp"), now), None);
        assert_eq!(
            last_good_age_seconds(Some("2025-12-31T23:58:30Z"), now),
            Some(90)
        );
        // A clock skew never renders a negative age.
        assert_eq!(
            last_good_age_seconds(Some("2026-01-01T00:00:05Z"), now),
            Some(0)
        );
    }

    #[test]
    fn a_discovery_pass_reports_an_association_change_only_when_it_made_one() {
        let mut summary = IdentityDiscoverySummary {
            considered: 3,
            identified: 1,
            unidentified: 2,
            ..IdentityDiscoverySummary::default()
        };
        assert!(!summary.changed_associations());
        summary.newly_linked = 1;
        assert!(summary.changed_associations());
        summary.newly_linked = 0;
        summary.closed_intervals = 1;
        assert!(summary.changed_associations());
    }

    #[test]
    fn association_effectiveness_requires_an_open_interval_and_an_active_node() {
        let record = |lifecycle: &str, validator_id: Option<&str>| NodeValidatorIdentityRecord {
            node_id: "0195f2a1-0000-4000-8000-000000000001".to_owned(),
            node_display_name: None,
            network_key: "platon-mainnet".to_owned(),
            lifecycle: lifecycle.to_owned(),
            state: Some("identified".to_owned()),
            observed_node_key: Some(format!("0x{}", "ab".repeat(64))),
            evaluated_at: Some("2026-01-01T00:00:00Z".to_owned()),
            validator_id: validator_id.map(str::to_owned),
            validator_node_key: validator_id.map(str::to_owned),
        };
        assert!(record("active", Some("validator-1")).association_effective());
        // An inactive Node keeps its recorded interval without projecting it.
        assert!(!record("inactive", Some("validator-1")).association_effective());
        assert!(!record("active", None).association_effective());
    }

    #[test]
    fn observed_p2p_public_key_requires_the_full_key() {
        let key = full_node_key(0xab);
        assert_eq!(observed_p2p_public_key(&enode_uri(&key)).unwrap(), key);
        // A key carried without a host is still the full observed key.
        assert_eq!(
            observed_p2p_public_key(&format!("enode://{}", &key[2..])).unwrap(),
            key
        );
        // Uppercase hex normalizes to the canonical lowercase form.
        assert_eq!(
            observed_p2p_public_key(&format!("enode://{}@h", "AB".repeat(64))).unwrap(),
            key
        );
        // A shortened key, a wrong scheme, or a missing key is never truncated
        // into a lookup candidate.
        assert!(observed_p2p_public_key("enode://abcd@h").is_err());
        assert!(observed_p2p_public_key(&format!("http://{}@h", &key[2..])).is_err());
        assert!(observed_p2p_public_key("enode://@h").is_err());
    }

    #[test]
    fn current_validator_status_follows_the_evidence_predicates() {
        let view = |outcome: Option<&str>, activity: Option<&str>| {
            current_validator_status(outcome, activity, "fresh")
        };
        assert_eq!(
            view(Some("success"), Some("active")).status,
            CurrentValidatorStatus::Validator
        );
        // A candidate is a currently valid staking identity: it stays Validator,
        // it is only a distinct Activity from Active.
        assert_eq!(
            view(Some("success"), Some("candidate")).status,
            CurrentValidatorStatus::Validator
        );
        assert_eq!(view(Some("success"), Some("producing")).state, "current");
        assert_eq!(
            view(Some("success"), Some("exiting")).qualifier,
            Some(CurrentValidatorQualifier::Exiting)
        );
        assert_eq!(
            view(Some("success"), Some("locked")).qualifier,
            Some(CurrentValidatorQualifier::Locked)
        );
        assert_eq!(
            view(Some("success"), Some("exited")).status,
            CurrentValidatorStatus::NotValidator
        );
        // Verifying is a candidate in a consensus round: a currently valid
        // staking identity, so Validator (Owner decision 2026-10-08).
        assert_eq!(
            view(Some("success"), Some("verifying")).status,
            CurrentValidatorStatus::Validator
        );
        // A success without an Activity, an unrecognized status, or a status 0
        // with a non-empty identifier cannot be fabricated into a negative.
        assert_eq!(
            view(Some("success"), None).status,
            CurrentValidatorStatus::Unknown
        );
        // Authoritative absence is negative; HTTP 404 is not.
        assert_eq!(
            view(Some("empty"), None).status,
            CurrentValidatorStatus::NotValidator
        );
        assert_eq!(
            view(Some("not_found"), None).status,
            CurrentValidatorStatus::Unknown
        );
        // A failed refresh retains the last-good verdict but marks it stale.
        let stale = current_validator_status(Some("error"), Some("active"), "stale");
        assert_eq!(stale.status, CurrentValidatorStatus::Validator);
        assert_eq!(stale.state, "stale");
        assert_eq!(
            current_validator_status(Some("error"), None, "unknown").status,
            CurrentValidatorStatus::Unknown
        );
        // Unconfigured or unsupported coverage never establishes a verdict,
        // even with a retained last-good Activity.
        assert_eq!(
            current_validator_status(Some("not_configured"), None, "unknown").status,
            CurrentValidatorStatus::Unknown
        );
        assert_eq!(
            current_validator_status(Some("unsupported"), Some("active"), "unknown").status,
            CurrentValidatorStatus::Unknown
        );
    }

    #[tokio::test]
    async fn discovery_identifies_the_full_key_and_is_idempotent() {
        let (_dir, db) = test_db().await;
        let key = full_node_key(0x11);
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.identified, 1);
        assert_eq!(summary.newly_linked, 1);
        assert_eq!(summary.unidentified, 0);
        assert_eq!(identity_state(&db, "node-1").await, "identified");
        let validator_node_id: String = sqlx::query_scalar(
            "SELECT validator_node_id FROM validators WHERE network_key = 'platon-mainnet'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(validator_node_id, key);
        let (origin, valid_until): (String, Option<String>) = sqlx::query_as(
            "SELECT origin, valid_until FROM node_validator_links WHERE node_id = 'node-1'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(origin, "automatic");
        assert_eq!(valid_until, None);

        // A second pass over the same key must not open a duplicate interval.
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.newly_linked, 0);
        let links: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_validator_links WHERE node_id = 'node-1'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(links, 1);
    }

    #[tokio::test]
    async fn discovery_records_a_reason_instead_of_guessing() {
        let (_dir, db) = test_db().await;
        let key = full_node_key(0x31);

        // No observation at all.
        discover_automatic_links(&db).await.unwrap();
        assert_eq!(
            identity_state(&db, "node-1").await,
            "network_identity_missing"
        );

        // Matching identity but no observed enode.
        set_chain_observation(&db, "node-1", REGISTERED_GENESIS, 1, 1, "lat", None).await;
        discover_automatic_links(&db).await.unwrap();
        assert_eq!(identity_state(&db, "node-1").await, "missing_public_key");

        // A malformed enode is not truncated into a lookup candidate.
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some("enode://abcd@host"),
        )
        .await;
        discover_automatic_links(&db).await.unwrap();
        assert_eq!(identity_state(&db, "node-1").await, "invalid_public_key");

        // An observed Network Identity that does not match the registered
        // Network is never searched against another Network.
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            999,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.unidentified, 1);
        assert_eq!(
            identity_state(&db, "node-1").await,
            "network_identity_mismatch"
        );
        let validators: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validators")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(validators, 0);
        let links: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_validator_links")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(links, 0);

        // A legacy manual Link is never a fallback for a missing observation.
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('legacy', 'platon-mainnet', '0xlegacy', NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, created_at, updated_at) VALUES ('legacy-link', 'node-1', 'legacy', 'primary', 'manual', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();
        discover_automatic_links(&db).await.unwrap();
        let automatic: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_validator_links WHERE node_id = 'node-1' AND origin = 'automatic'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            automatic, 0,
            "a manual link is never the automatic fallback"
        );
    }

    #[tokio::test]
    async fn discovery_closes_the_old_interval_on_a_chain_key_change() {
        let (_dir, db) = test_db().await;
        let key_a = full_node_key(0x21);
        let key_b = full_node_key(0x22);
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key_a)),
        )
        .await;
        discover_automatic_links(&db).await.unwrap();

        // The chain key changes: the old association interval ends and the new
        // identity is identified without merging the two Validators.
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key_b)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.closed_intervals, 1);
        assert_eq!(summary.newly_linked, 1);

        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT v.validator_node_id, l.valid_until FROM node_validator_links l JOIN validators v ON v.validator_id = l.validator_id WHERE l.node_id = 'node-1' AND l.origin = 'automatic' ORDER BY l.valid_from, l.link_id",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, key_a);
        assert!(rows[0].1.is_some(), "the old interval must be closed");
        assert_eq!(rows[1].0, key_b);
        assert_eq!(rows[1].1, None);
        let validators: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM validators WHERE network_key = 'platon-mainnet'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(validators, 2, "the two chain identities stay independent");
    }

    #[tokio::test]
    async fn automatic_and_manual_overlap_triggers_are_partitioned_by_origin() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xmanual", None, &owner_id)
            .await
            .unwrap();
        create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "primary",
            "2026-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        // A legacy manual interval must not block automatic discovery, and the
        // automatic interval must not collide with it.
        let key = full_node_key(0x41);
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.newly_linked, 1);
        let origins: Vec<String> = sqlx::query_scalar(
            "SELECT origin FROM node_validator_links WHERE node_id = 'node-1' ORDER BY origin",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(origins, vec!["automatic".to_owned(), "manual".to_owned()]);
    }

    #[tokio::test]
    async fn discovery_ends_the_interval_on_contradiction_but_keeps_last_good_on_absence() {
        let (_dir, db) = test_db().await;
        let key = full_node_key(0x51);
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        discover_automatic_links(&db).await.unwrap();
        let open = || {
            let pool = db.pool().clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM node_validator_links WHERE node_id = 'node-1' AND origin = 'automatic' AND valid_until IS NULL",
                )
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(open().await, 1);

        // A transient loss of key evidence retains the last established
        // association; absence is not contradiction.
        set_chain_observation(&db, "node-1", REGISTERED_GENESIS, 1, 1, "lat", None).await;
        discover_automatic_links(&db).await.unwrap();
        assert_eq!(
            open().await,
            1,
            "missing evidence must not drop a last-good association"
        );
        assert_eq!(identity_state(&db, "node-1").await, "missing_public_key");

        // An observed Network Identity that contradicts the registered Network
        // ends the old interval instead of projecting it as a fallback.
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            999,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.closed_intervals, 1);
        assert_eq!(
            open().await,
            0,
            "contradictory evidence must end the old association"
        );
        assert_eq!(
            identity_state(&db, "node-1").await,
            "network_identity_mismatch"
        );
    }

    /// A raw pool at the schema immediately before the one-time #174 Validator
    /// model cutover (0053), for seeding a legacy generation.
    async fn schema_before_validator_model_migration(path: &std::path::Path) -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        migrator_through(52).run(&pool).await.unwrap();
        pool
    }

    /// Seed one Validator generation: a Link on node-1 whose `origin` is either
    /// the legacy `manual` model or the automatic #173 model, plus a current
    /// snapshot, ranking/counter history and daily/monthly aggregates.
    async fn seed_validator_generation(
        pool: &sqlx::SqlitePool,
        validator_id: &str,
        key: &str,
        origin: &str,
    ) {
        let now = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (?, 'platon-mainnet', ?, NULL, ?, ?)")
            .bind(validator_id)
            .bind(format!("0x{validator_id}"))
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES (?, 'node-1', ?, ?, ?, ?, NULL, ?, ?)")
            .bind(format!("link-{validator_id}"))
            .bind(validator_id)
            .bind(if origin == "manual" { Some("primary") } else { None })
            .bind(origin)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, stake_amount, reward_amount, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, last_observation_key, updated_at) VALUES (?, 'platscan', 'success', ?, 'active', ?, ?, ?, '10', '5', 100, 'counter_reset', 'ranking_changed', 10, 20, 1, ?, ?, ?)")
            .bind(validator_id)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(key)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO validator_ranking_history (history_id, validator_id, previous_rank, current_rank, observed_at, observation_key) VALUES (?, ?, 10, 20, ?, ?)")
            .bind(format!("rank-{validator_id}"))
            .bind(validator_id)
            .bind(now)
            .bind(key)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO validator_counter_history (history_id, validator_id, counter_name, previous_value, current_value, observed_at, observation_key) VALUES (?, ?, 'reward_amount', '5', '1', ?, ?)")
            .bind(format!("counter-{validator_id}"))
            .bind(validator_id)
            .bind(now)
            .bind(key)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, source, observation_key) VALUES (?, ?, 'UTC', '2026-01-01', '2026-01', ?, ?, 'platscan', ?)")
            .bind(format!("daily-{validator_id}"))
            .bind(validator_id)
            .bind(now)
            .bind(now)
            .bind(key)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO validator_monthly_aggregates (aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, updated_at) VALUES (?, ?, 'UTC', '2026-01', 1, ?, ?, ?)")
            .bind(format!("monthly-{validator_id}"))
            .bind(validator_id)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn seed_legacy_network_agent_and_node(pool: &sqlx::SqlitePool) {
        let now = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('platon-mainnet', 'Mainnet', '0x0', 1, 1, 'lat', ?, ?)")
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-1', 1, ?, ?)")
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, lifecycle, visibility, inventory_revision, first_seen_at, updated_at, rpc_endpoint) VALUES ('node-1', 'agent-1', 'platon-mainnet', 'active', 'private', 1, ?, ?, 'http://127.0.0.1:1')")
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    }

    #[cfg(unix)]
    fn restrict_database_permissions(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// #174: the cutover deletes the legacy manual Links and the old Validator
    /// snapshots/history/daily/monthly aggregates, but leaves Node monitoring
    /// history, existing Incident evidence and Audit in place.
    #[tokio::test]
    async fn migration_0053_removes_the_legacy_generation_and_preserves_unrelated_evidence() {
        let now = "2026-01-01T00:00:00Z";
        let dir = tempdir().unwrap();
        let path = dir.path().join("server.db");
        let pool = schema_before_validator_model_migration(&path).await;
        seed_legacy_network_agent_and_node(&pool).await;
        seed_validator_generation(&pool, "validator-manual", "obs-manual", "manual").await;
        // An automatic generation is the new model and must survive #174.
        seed_validator_generation(&pool, "validator-auto", "obs-auto", "automatic").await;
        // Node monitoring history must be preserved.
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES ('node-1', 10, 10, 20, 3, ?)")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, opened_at) VALUES ('node-1', 'peer-1', 'inbound', 0, 0, 0, ?)")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_metric_samples (node_id, metric, observed_at, received_at, value) VALUES ('node-1', 'process_cpu_percent', ?, ?, 1.5)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        // Existing Incident and Audit evidence must be retained. The rule row
        // only satisfies the Incident foreign key; the catalog is seeded later.
        sqlx::query("INSERT INTO alert_rules (rule_key, enabled, severity, version, condition_json, created_at, updated_at) VALUES ('validator.counter_reset', 1, 'critical', 1, '{}', ?, ?)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('incident-1', 'validator.counter_reset', 1, 'validator', 'validator-manual', 'critical', 'open', 1, ?, '{}')")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, created_at) VALUES (NULL, 'validator_created', 'validator', 'validator-manual', ?)")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        #[cfg(unix)]
        restrict_database_permissions(&path);

        let db = initialize(ServerDatabaseConfig::new(&path)).await.unwrap();

        let origins: Vec<String> =
            sqlx::query_scalar("SELECT origin FROM node_validator_links ORDER BY origin")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert_eq!(origins, vec!["automatic".to_owned()]);
        let validators: Vec<String> =
            sqlx::query_scalar("SELECT validator_id FROM validators ORDER BY validator_id")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert_eq!(
            validators,
            vec!["validator-auto".to_owned()],
            "only the legacy generation is removed"
        );
        for table in [
            "current_validator_insights",
            "validator_ranking_history",
            "validator_counter_history",
            "validator_daily_snapshots",
            "validator_monthly_aggregates",
        ] {
            let rows: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT validator_id FROM {table} ORDER BY validator_id"
            ))
            .fetch_all(db.pool())
            .await
            .unwrap();
            assert_eq!(
                rows,
                vec!["validator-auto".to_owned()],
                "{table} must keep the automatic generation and drop the legacy one"
            );
        }
        for table in [
            "block_history_state",
            "peer_presence_intervals",
            "node_metric_samples",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(count, 1, "{table} is Node history and must survive");
        }
        let incidents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_incidents")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(incidents, 1, "existing Incident evidence must survive");
        let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(audits, 1, "necessary Audit evidence must survive");
        let marker: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM validator_model_migration WHERE migration_key = 'validator_model'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(marker, 1, "the cutover marker must be recorded");
        assert_eq!(
            db.schema_version().await.unwrap(),
            crate::database::SERVER_SCHEMA_VERSION
        );
    }

    /// #174: removing a legacy generation with an open counter-reset Incident
    /// must not be read as a recovery, and a later Provider pass must not
    /// re-open the generation that was removed.
    #[tokio::test]
    async fn legacy_generation_removal_does_not_resolve_open_incidents_or_reaccumulate() {
        let now = "2026-01-01T00:00:00Z";
        let dir = tempdir().unwrap();
        let path = dir.path().join("server.db");
        let pool = schema_before_validator_model_migration(&path).await;
        seed_legacy_network_agent_and_node(&pool).await;
        seed_validator_generation(&pool, "validator-legacy", "obs-1", "manual").await;
        // An open counter-reset Incident and its firing evaluation state belong
        // to the legacy generation and must survive as evidence.
        sqlx::query("INSERT INTO alert_rules (rule_key, enabled, severity, version, condition_json, created_at, updated_at) VALUES ('validator.counter_reset', 1, 'critical', 1, '{}', ?, ?)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO alert_rule_state (rule_key, subject_kind, subject_key, state, since, input_kind, last_evaluated_at) VALUES ('validator.counter_reset', 'validator', 'validator-legacy', 'firing', ?, 'known', ?)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('incident-legacy', 'validator.counter_reset', 1, 'validator', 'validator-legacy', 'critical', 'open', 1, ?, '{}')")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        #[cfg(unix)]
        restrict_database_permissions(&path);

        let db = initialize(ServerDatabaseConfig::new(&path)).await.unwrap();

        let validators: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validators")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(validators, 0, "the legacy generation is removed");
        let snapshots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validator_daily_snapshots")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(snapshots, 0);
        // The Provider pass now sees no Validator to refresh, so it cannot
        // recreate the removed generation.
        let provider = FakeProvider::default();
        let summary = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(summary.attempted, 0);
        assert!(
            load_insight(&db, "validator-legacy")
                .await
                .unwrap()
                .is_none()
        );
        // The reference boundary is closed too: an old insight cannot be
        // re-inserted for a removed identity.
        let reinsert = sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, last_attempt_received_at, updated_at) VALUES ('validator-legacy', 'platscan', 'success', ?, ?)")
            .bind(now)
            .bind(now)
            .execute(db.pool())
            .await;
        assert!(
            reinsert.is_err(),
            "a removed identity must not be re-openable"
        );
        // The cutover must not be read as a recovery: the Incident and its
        // firing evaluation state stay exactly as they were.
        let incident: (String, String) = sqlx::query_as(
            "SELECT state, subject_key FROM alert_incidents WHERE incident_id = 'incident-legacy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(incident, ("open".to_owned(), "validator-legacy".to_owned()));
        let firing: String = sqlx::query_scalar(
            "SELECT state FROM alert_rule_state WHERE rule_key = 'validator.counter_reset' AND subject_key = 'validator-legacy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(firing, "firing");
    }

    /// #174: a failure after the destructive statements must roll the whole
    /// migration back and stop startup, never leaving a half-migrated database.
    #[tokio::test]
    async fn a_failed_validator_model_migration_rolls_back_instead_of_serving_half_migrated_state()
    {
        let dir = tempdir().unwrap();
        let path = dir.path().join("server.db");
        let pool = schema_before_validator_model_migration(&path).await;
        seed_legacy_network_agent_and_node(&pool).await;
        seed_validator_generation(&pool, "validator-legacy", "obs-1", "manual").await;
        // Sabotage the marker table so the migration's final INSERT fails only
        // after every deletion has run, exercising rollback rather than a
        // failure before the destructive statements.
        sqlx::query("CREATE TABLE validator_model_migration (migration_key TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        #[cfg(unix)]
        restrict_database_permissions(&path);

        let error = match initialize(ServerDatabaseConfig::new(&path)).await {
            Ok(_) => panic!("a failed cutover must not return a usable database"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            crate::database::ServerDatabaseError::Migration(_)
        ));

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&path)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        let version: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, 52, "a failed migration must not be recorded");
        let links: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM node_validator_links WHERE origin = 'manual'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            links, 1,
            "the manual generation must survive a failed cutover"
        );
        let snapshots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validator_daily_snapshots")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(snapshots, 1);
        let marker: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validator_model_migration")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(marker, 0);
    }
}
