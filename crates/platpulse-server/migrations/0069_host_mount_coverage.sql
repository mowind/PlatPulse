-- Issue #216: the mount coverage read.
--
-- The storage evidence an Agent reports is stored per (Agent, metric, mount
-- path), so the list of mount paths is what makes that evidence reachable at
-- all. The ledger is the source of that list - a path the Agent reported once
-- keeps being answered after its samples are released - and the coverage route
-- reads one Agent's storage series newest path first, with a limit, so the read
-- must be served by an index instead of a sort of every mount path the Agent
-- has ever reported.
--
-- The leading columns are the series prefix the read is bound by, and the
-- trailing pair is the read's own order: (last_observed_at DESC, dimension
-- ASC) with equality on (agent_id, metric), which is what lets the limit drop
-- the oldest paths rather than an arbitrary slice of the mount churn.
CREATE INDEX host_metric_series_state_mount_idx
    ON host_metric_series_state (agent_id, metric, last_observed_at DESC, dimension ASC);
