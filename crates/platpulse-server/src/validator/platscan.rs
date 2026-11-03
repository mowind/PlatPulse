//! PlatScan Validator Provider adapter and its response normalization.

use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::BTreeMap;

use super::decimal::{decimal_exceeds_max, validate_nonnegative_decimal};
use super::provider::{
    MAX_PROVIDER_BODY_LEN, MAX_RANKING_COHORT, MAX_RANKING_PAGES, NetworkRanking,
    RANKING_PAGE_SIZE, RankingProviderResult, ValidatorActivity, ValidatorObservation,
    ValidatorProvider, ValidatorProviderResult,
};

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

pub(super) fn normalize_bounded_text(value: &str) -> Result<Option<String>, String> {
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

#[cfg(test)]
mod tests {

    use crate::network::create_network;

    use std::collections::BTreeMap;

    use serde_json::Value;

    use crate::validator::insight::load_insight;
    use crate::validator::provider::{
        MAX_PROVIDER_BODY_LEN, MAX_RANKING_COHORT, RANKING_PAGE_SIZE, RankingProviderResult,
        ValidatorActivity, ValidatorObservation, ValidatorProviderResult,
    };
    use crate::validator::refresh::{refresh_all, validate_observation};
    use crate::validator::registry::{create_link, create_validator};
    use crate::validator::test_helpers::{
        deployments, platscan_success, provider_node_id, start_mock_platscan, test_db,
    };

    use super::*;

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

    /// Replay the unmodified mainnet HTTP bodies, not a synthetic approximation.
    /// Capture requests, hashes, date and pinned source reconciliation are in
    /// tests/fixtures/platscan-mainnet/provenance.json and
    /// docs/design/platscan-mainnet-validation.md. CI never contacts PlatScan.
    #[tokio::test]
    async fn platscan_mainnet_capture_reaches_public_api_and_retains_independent_last_good() {
        use axum::body::to_bytes;
        use axum::extract::State;

        const DETAIL: &[u8] =
            include_bytes!("../../tests/fixtures/platscan-mainnet/staking-details.json");
        const PAGES: [&[u8]; 5] = [
            include_bytes!("../../tests/fixtures/platscan-mainnet/alive-staking-list-page-1.json"),
            include_bytes!("../../tests/fixtures/platscan-mainnet/alive-staking-list-page-2.json"),
            include_bytes!("../../tests/fixtures/platscan-mainnet/alive-staking-list-page-3.json"),
            include_bytes!("../../tests/fixtures/platscan-mainnet/alive-staking-list-page-4.json"),
            include_bytes!("../../tests/fixtures/platscan-mainnet/alive-staking-list-page-5.json"),
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
        sqlx::query("UPDATE node_validator_links SET origin = 'automatic' WHERE link_id = $1")
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
            "../../tests/fixtures/platscan-validator-status/staking-details-status-6.json"
        );
        const PAGES: [&[u8]; 5] = [
            include_bytes!(
                "../../tests/fixtures/platscan-validator-status/alive-staking-list-page-1.json"
            ),
            include_bytes!(
                "../../tests/fixtures/platscan-validator-status/alive-staking-list-page-2.json"
            ),
            include_bytes!(
                "../../tests/fixtures/platscan-validator-status/alive-staking-list-page-3.json"
            ),
            include_bytes!(
                "../../tests/fixtures/platscan-validator-status/alive-staking-list-page-4.json"
            ),
            include_bytes!(
                "../../tests/fixtures/platscan-validator-status/alive-staking-list-page-5.json"
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
        sqlx::query("UPDATE node_validator_links SET origin = 'automatic' WHERE link_id = $1")
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
}
