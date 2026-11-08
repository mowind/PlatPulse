//! Server-managed Validator identities and explicit Node Validator Links.
//!
//! Validators are keyed by `(Network, validator_node_id)` and are never
//! inferred from Agent reports, consensus membership, provider data, or
//! Node identity. Link mutations are transactional with their Audit Event and
//! reject every temporal overlap for one Node before inserting or updating.

mod analytics;
mod decimal;
mod error;
mod identity;
mod insight;
#[cfg(test)]
mod migration_tests;
mod platscan;
mod provider;
mod refresh;
mod registry;
mod status;
#[cfg(test)]
mod test_helpers;

pub use analytics::{
    TREND_DEFAULT_LIMIT, TREND_DEFAULT_WINDOW_DAYS, TREND_MAX_ASSOCIATIONS, TREND_MAX_LIMIT,
    TREND_MAX_WINDOW_DAYS, ValidatorAssociationRecord, ValidatorDailySnapshotRecord,
    ValidatorMonthlyAggregateRecord, ValidatorTrendGap, ValidatorTrendMonth, ValidatorTrendPage,
    ValidatorTrendPoint, ValidatorTrendQuery, analytics_period, count_deleted_nodes,
    list_daily_snapshots, list_monthly_aggregates, list_validator_associations, load_daily_trend,
    local_date_at, local_day_bounds, local_period_at,
};
pub use decimal::{accumulate_decimal, cumulative_block_rate};
pub use error::ValidatorError;
pub use identity::{
    EffectiveAutomaticLink, IdentityDiscoverySummary, LinkScope, NodeValidatorIdentityRecord,
    automatic_identity_reason, discover_automatic_links, effective_automatic_links,
    list_node_validator_identities, observed_p2p_public_key,
};
pub use insight::{
    ValidatorCounterHistoryRecord, ValidatorInsightRecord, ValidatorLinkContextRecord,
    ValidatorRankingHistoryRecord, list_counter_history, list_insights, list_link_context_at,
    list_ranking_history, load_insight,
};
pub use platscan::{
    MAX_PROVIDER_NETWORK_KEY_LEN, MAX_PROVIDER_NETWORKS, PlatScanValidatorProvider,
    is_platscan_node_id, normalize_provider_base_url, validate_provider_deployments,
};
pub use provider::{
    DisabledValidatorProvider, MAX_PROVIDER_BODY_LEN, MAX_PROVIDER_DIAGNOSTIC_LEN,
    MAX_RANKING_COHORT, MAX_RANKING_PAGES, NetworkRanking, RANKING_PAGE_SIZE,
    RankingProviderResult, SharedValidatorProvider, ValidatorActivity, ValidatorObservation,
    ValidatorProvider, ValidatorProviderResult,
};
pub use refresh::{
    RefreshSummary, refresh_all, refresh_all_with_channels, refresh_all_with_channels_in_timezone,
};
pub use registry::{
    MAX_VALIDATOR_DISPLAY_NAME_LEN, MAX_VALIDATOR_NODE_ID_LEN, NodeValidatorLinkRecord,
    ValidatorRecord, create_link, create_validator, end_link, get_link, get_validator, list_links,
    list_validators, update_link, validate_display_name, validate_role, validate_validator_node_id,
    validate_validity,
};
pub use status::{
    CurrentValidatorQualifier, CurrentValidatorStatus, CurrentValidatorStatusView,
    current_validator_status, freshness, last_good_age_seconds, project_activity, project_verdict,
};
