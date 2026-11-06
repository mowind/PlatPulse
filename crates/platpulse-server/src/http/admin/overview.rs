//! Admin overview: Agent, Node and Network summaries plus Attention.
use super::*;
use crate::node_currency;

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminOverview {
    pub generated_at: String,
    pub summary: AdminOverviewSummary,
    /// Server-owned attention queue. The WebUI presents these items and
    /// never recomputes health policy or attention in the browser.
    pub attention: Vec<AttentionItem>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminOverviewSummary {
    pub agents: AgentSummary,
    pub nodes: NodeSummary,
    pub networks: NetworkSummary,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentSummary {
    pub total: i64,
    pub online: i64,
    pub offline: i64,
    pub unknown: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeSummary {
    pub total: i64,
    pub active: i64,
    pub healthy: i64,
    pub unhealthy: i64,
    pub unknown: i64,
    /// Nodes in a non-active lifecycle (e.g. retired); not part of the
    /// health buckets because no observation policy applies to them.
    pub retired: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NetworkSummary {
    pub total: i64,
    pub with_identity_mismatch: i64,
}

impl AdminOverviewSummary {
    fn checked(agents: AgentSummary, nodes: NodeSummary, networks: NetworkSummary) -> Option<Self> {
        let agent_buckets = agents.online + agents.offline + agents.unknown;
        let active_node_buckets = nodes.healthy + nodes.unhealthy + nodes.unknown;
        (agents.total == agent_buckets
            && nodes.active == active_node_buckets
            && nodes.total == nodes.active + nodes.retired
            && networks.with_identity_mismatch <= networks.total)
            .then_some(Self {
                agents,
                nodes,
                networks,
            })
    }
}

/// Count registered Networks and the distinct subset with confirmed identity
/// mismatch evidence. A mismatch is confirmed only while the identity
/// component is ok, has a successful value, and all four identity fields
/// are present. Retired Nodes and retained last-good values behind a failed,
/// disabled, unsupported, starting, or otherwise non-ok component do not
/// contribute.
async fn overview_network_summary(pool: &sqlx::AnyPool) -> Result<NetworkSummary, sqlx::Error> {
    let (total, with_identity_mismatch) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT COUNT(*) FROM networks), (SELECT COUNT(*) FROM networks n WHERE EXISTS (SELECT 1 FROM nodes nd JOIN current_node_chain_observations c ON c.node_id = nd.node_id JOIN component_status identity ON identity.node_id = nd.node_id AND identity.component_key = 'network_identity' WHERE nd.network_key = n.network_key AND nd.lifecycle = 'active' AND identity.state = 'ok' AND identity.value_revision > 0 AND c.network_genesis_hash IS NOT NULL AND c.network_chain_id IS NOT NULL AND c.network_p2p_network_id IS NOT NULL AND c.network_address_hrp IS NOT NULL AND (c.network_genesis_hash != n.genesis_hash OR c.network_chain_id != n.chain_id OR c.network_p2p_network_id != n.p2p_network_id OR c.network_address_hrp != n.address_hrp)))",
    )
    .fetch_one(pool)
    .await?;
    Ok(NetworkSummary {
        total,
        with_identity_mismatch,
    })
}

fn severity_rank(severity: AttentionSeverity) -> u8 {
    match severity {
        AttentionSeverity::Critical => 0,
        AttentionSeverity::Warning => 1,
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/overview",
    tag = "admin",
    responses((status = 200, description = "Server-owned attention queue and overview summary", body = AdminOverview), (status = 500, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn overview(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let generated_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    // A database failure is a Server failure: it must surface as an error
    // envelope, never as an authoritative empty queue (webui.md §5.3).
    let agents = match sqlx::query_as::<_, crate::attention::AgentAttentionRow>(&format!(
        "{} ORDER BY a.agent_id",
        crate::attention::AGENT_ATTENTION_SELECT
    ))
    .fetch_all(state.db().pool())
    .await
    {
        Ok(agents) => agents,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut agent_summary = AgentSummary {
        total: agents.len() as i64,
        online: 0,
        offline: 0,
        unknown: 0,
    };
    let mut attention: Vec<AttentionItem> = Vec::new();
    for agent in &agents {
        match agent_liveness(agent.last_received_at.as_deref()) {
            "online" => agent_summary.online += 1,
            "offline" => agent_summary.offline += 1,
            _ => agent_summary.unknown += 1,
        }
        // The Agent Attention Items come from the Server-owned occurrence
        // boundary, which drops acknowledged occurrences (issue #172).
        match crate::attention::evaluate_agent(state.db().pool(), agent).await {
            Ok(items) => attention.extend(items),
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        }
    }
    let network_summary = match overview_network_summary(state.db().pool()).await {
        Ok(summary) => summary,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let rows = match sqlx::query_as::<_, (String, String, Option<String>, String, i64, String, String)>(
        "SELECT node_id, network_key, display_name, lifecycle, inventory_revision, visibility, first_seen_at FROM nodes ORDER BY node_id",
    )
    .fetch_all(state.db().pool())
    .await
    {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut node_summary = NodeSummary {
        total: rows.len() as i64,
        active: 0,
        healthy: 0,
        unhealthy: 0,
        unknown: 0,
        retired: 0,
    };
    for (
        node_id,
        network_key,
        display_name,
        lifecycle,
        inventory_revision,
        visibility,
        first_seen_at,
    ) in rows
    {
        let assessed = match node_diagnostic(
            &state,
            node_id.clone(),
            network_key.clone(),
            display_name.clone(),
            lifecycle.clone(),
            inventory_revision,
            visibility.clone(),
        )
        .await
        {
            Ok(assessed) => assessed,
            Err(_) => return diagnostics_unavailable(&request_id),
        };
        if lifecycle != "active" {
            node_summary.retired += 1;
            continue;
        }
        node_summary.active += 1;
        // First-observation grace is an Attention rule, not a Health state:
        // the Node still counts as Unknown, it only raises no prompt yet.
        let in_grace = first_observation_in_grace(&first_seen_at, &assessed);
        if assessed.identity_disposition == node_currency::NodeIdentity::Mismatched {
            let observed_at = sqlx::query_scalar::<_, Option<String>>(
                "SELECT COALESCE((SELECT observed_at FROM component_status WHERE node_id = $1 AND component_key = 'network_identity'), (SELECT updated_at FROM current_node_chain_observations WHERE node_id = $2))",
            )
            .bind(&node_id)
            .bind(&node_id)
            .fetch_optional(state.db().pool())
            .await;
            let observed_at = match observed_at {
                Ok(observed_at) => observed_at.flatten(),
                Err(_) => return diagnostics_unavailable(&request_id),
            };
            attention.push(AttentionItem {
                id: format!("node_identity_mismatch:node:{node_id}"),
                kind: AttentionKind::NodeIdentityMismatch,
                severity: AttentionSeverity::Critical,
                subject_kind: AttentionSubjectKind::Node,
                subject_id: node_id.clone(),
                subject_label: display_name.clone().unwrap_or_else(|| node_id.clone()),
                message: "the Node identity mismatches the registered Network".to_owned(),
                observed_at,
                // Node prompts are never acknowledgeable; the Server-owned
                // evidence boundary is Agent-only (issue #172).
                evidence_key: String::new(),
            });
        }
        // The Server-clock instant Node Currency was judged on, so the prompt
        // and the Health it explains name the same evidence.
        let observed_at = assessed.currency_as_of.clone();
        // A Node Unknown because of the mismatch already raised its Critical
        // prompt above; one Unknown for another reason still raises its own.
        let explained_by_mismatch =
            assessed.health_reason == node_currency::HealthReason::IdentityMismatch;
        let diagnostic = assessed.diagnostic;
        match diagnostic.health.as_str() {
            "healthy" => node_summary.healthy += 1,
            "unhealthy" => {
                node_summary.unhealthy += 1;
                attention.push(AttentionItem {
                    id: format!("node_unhealthy:node:{node_id}"),
                    kind: AttentionKind::NodeUnhealthy,
                    severity: AttentionSeverity::Critical,
                    subject_kind: AttentionSubjectKind::Node,
                    subject_id: node_id.clone(),
                    subject_label: display_name.unwrap_or_else(|| node_id.clone()),
                    message: diagnostic.health_reason.clone(),
                    observed_at: observed_at.clone(),
                    evidence_key: String::new(),
                });
            }
            _ if in_grace || explained_by_mismatch => {
                node_summary.unknown += 1;
            }
            _ => {
                node_summary.unknown += 1;
                attention.push(AttentionItem {
                    id: format!("node_health_unknown:node:{node_id}"),
                    kind: AttentionKind::NodeHealthUnknown,
                    severity: AttentionSeverity::Warning,
                    subject_kind: AttentionSubjectKind::Node,
                    subject_id: node_id.clone(),
                    subject_label: display_name.unwrap_or_else(|| node_id.clone()),
                    message: diagnostic.health_reason.clone(),
                    observed_at: observed_at.clone(),
                    evidence_key: String::new(),
                });
            }
        }
        if diagnostic.resync_state != "normal" {
            let resync_observed_at = match sqlx::query_scalar::<_, String>(
                "SELECT COALESCE(resync_last_progress_at, updated_at) FROM block_history_state WHERE node_id = $1",
            )
            .bind(&node_id)
            .fetch_optional(state.db().pool())
            .await
            {
                Ok(instant) => instant.or(observed_at.clone()),
                Err(_) => return diagnostics_unavailable(&request_id),
            };
            attention.push(AttentionItem {
                id: format!("node_resync:node:{node_id}"),
                kind: AttentionKind::NodeResync,
                severity: AttentionSeverity::Warning,
                subject_kind: AttentionSubjectKind::Node,
                subject_id: node_id.clone(),
                subject_label: node_id.clone(),
                message: format!("the Node resync state is {}", diagnostic.resync_state),
                observed_at: resync_observed_at,
                evidence_key: String::new(),
            });
        }
    }
    attention.sort_by(|a, b| {
        severity_rank(a.severity)
            .cmp(&severity_rank(b.severity))
            .then_with(
                || match (a.observed_at.as_deref(), b.observed_at.as_deref()) {
                    (Some(left), Some(right)) => {
                        crate::auth::parse_rfc3339(right).cmp(&crate::auth::parse_rfc3339(left))
                    }
                    // Items with an authoritative observation time sort before
                    // items whose observation time is unknown.
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                },
            )
            .then_with(|| a.subject_label.cmp(&b.subject_label))
            .then_with(|| a.id.cmp(&b.id))
    });
    let Some(summary) = AdminOverviewSummary::checked(agent_summary, node_summary, network_summary)
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::INTERNAL_SERVER_ERROR,
            "overview_invariant_failed",
            "server overview invariant failed",
        );
    };
    Json(AdminOverview {
        generated_at,
        summary,
        attention,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AuthenticatedSession;
    use crate::http::admin::test_helpers::*;
    use axum::body::to_bytes;
    use serde_json::Value;
    use tempfile::tempdir;
    use time::OffsetDateTime;

    /// A contradicted identity leaves the Node Unknown and raises only its
    /// Critical mismatch prompt, never a second Health Unknown prompt.
    #[tokio::test]
    async fn an_identity_mismatch_raises_only_the_mismatch_prompt() {
        let (_dir, state) = node_inventory_state().await;
        sqlx::query("UPDATE component_status SET state = 'ok', error_code = NULL, error_message = NULL WHERE node_id = 'node-mismatched'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = overview(
            State(state),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["summary"]["nodes"]["active"], 2);
        assert_eq!(value["summary"]["nodes"]["healthy"], 1);
        assert_eq!(value["summary"]["nodes"]["unknown"], 1);
        let node_prompts = value["attention"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["subject_kind"] == "node")
            .map(|item| item["kind"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(node_prompts, ["node_identity_mismatch"]);
    }

    /// A mismatched Node that is Unknown because its evidence is stale keeps
    /// its Health Unknown prompt: the mismatch prompt does not explain it.
    #[tokio::test]
    async fn a_stale_mismatched_node_raises_both_prompts() {
        let (_dir, state) = node_inventory_state().await;
        sqlx::query("UPDATE component_status SET state = 'ok', error_code = NULL, error_message = NULL WHERE node_id = 'node-mismatched'")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("UPDATE component_status SET observation_at = '2000-01-01T00:00:00Z' WHERE node_id = 'node-mismatched' AND component_key = 'sync'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = overview(
            State(state),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["summary"]["nodes"]["unknown"], 1);
        let mut node_prompts = value["attention"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["subject_kind"] == "node")
            .map(|item| item["kind"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        node_prompts.sort();
        assert_eq!(
            node_prompts,
            ["node_health_unknown", "node_identity_mismatch"]
        );
    }

    #[tokio::test]
    async fn overview_reports_server_owned_attention_and_summary() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let now = crate::auth::now_utc();
        let fresh = crate::auth::format_rfc3339(now - time::Duration::seconds(20));
        let created = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at, last_received_at, shutdown_state, security_event_count) VALUES ('agent-ov', 1, $1, $2, $3, 'running', 0)")
            .bind(created)
            .bind(created)
            .bind(&fresh)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', $1, $2)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        for (node_id, name, visibility, rpc_state) in [
            ("node-a", "Node A", "public", "ok"),
            ("node-b", "Node B", "private", "error"),
        ] {
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, 'agent-ov', 'mainnet', $2, 'ws://127.0.0.1:1', 'active', $3, 1, $4, $5)")
                .bind(node_id)
                .bind(name)
                .bind(visibility)
                .bind(created)
                .bind(created)
                .execute(state.db().pool())
                .await
                .unwrap();
            sqlx::query("INSERT INTO current_node_chain_observations (node_id, current_block, syncing, updated_at) VALUES ($1, 100, 0, $2)")
                .bind(node_id)
                .bind(&fresh)
                .execute(state.db().pool())
                .await
                .unwrap();
            for component in ["rpc", "sync", "consensus", "network_identity"] {
                let component_state = if component == "rpc" { rpc_state } else { "ok" };
                sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, observation_at, state_revision, value_revision) VALUES ('agent-ov', 'node', $1, $2, $3, $4, $5, $6, $7, $7, 1, 1)")
                    .bind(node_id)
                    .bind(node_id)
                    .bind(component)
                    .bind(component_state)
                    .bind(&fresh)
                    .bind(&fresh)
                    .bind(&fresh)
                    .execute(state.db().pool())
                    .await
                    .unwrap();
            }
        }
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = overview(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["summary"]["nodes"]["total"], 2);
        assert_eq!(value["summary"]["nodes"]["active"], 2);
        assert_eq!(value["summary"]["nodes"]["healthy"], 1);
        assert_eq!(value["summary"]["nodes"]["unhealthy"], 1);
        assert_eq!(value["summary"]["nodes"]["unknown"], 0);
        assert_eq!(value["summary"]["nodes"]["retired"], 0);
        assert_eq!(value["summary"]["agents"]["total"], 1);
        assert_eq!(value["summary"]["agents"]["online"], 1);
        assert_eq!(value["summary"]["agents"]["offline"], 0);
        assert_eq!(value["summary"]["networks"]["total"], 1);
        assert_eq!(value["summary"]["networks"]["with_identity_mismatch"], 0);
        let attention = value["attention"].as_array().unwrap();
        assert_eq!(attention.len(), 1);
        assert_eq!(attention[0]["kind"], "node_unhealthy");
        assert_eq!(attention[0]["severity"], "critical");
        assert_eq!(attention[0]["subject_kind"], "node");
        assert_eq!(attention[0]["subject_id"], "node-b");
        assert_eq!(attention[0]["subject_label"], "Node B");
        assert_eq!(attention[0]["message"], "RPC observation failed");
    }

    #[tokio::test]
    async fn overview_flags_offline_agents_spool_loss_and_unknown_nodes() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let now = crate::auth::now_utc();
        let stale = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        let created = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at, last_received_at, shutdown_state, security_event_count) VALUES ('agent-ov2', 1, $1, $2, $3, 'running', 2)")
            .bind(created)
            .bind(created)
            .bind(&stale)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_host_observations (agent_id, spool_store_fatal, spool_dropped_sequence_to, updated_at) VALUES ('agent-ov2', 1, 5, $1)")
            .bind(&stale)
            .execute(state.db().pool())
            .await
            .unwrap();
        // The recorded gap interval has its own event time, distinct from the
        // Host observation, so the Server must not reuse one Agent-wide clock.
        let gap_time = crate::auth::format_rfc3339(now - time::Duration::hours(2));
        sqlx::query("INSERT INTO report_sequence_gaps (agent_id, boot_id, from_sequence, to_sequence, created_at) VALUES ('agent-ov2', 'boot-ov2', 3, 4, $1)")
            .bind(&gap_time)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', $1, $2)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-unknown', 'agent-ov2', 'mainnet', 'Node Unknown', 'ws://127.0.0.1:2', 'active', 'private', 1, $1, $2)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = overview(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["summary"]["agents"]["total"], 1);
        assert_eq!(value["summary"]["agents"]["online"], 0);
        assert_eq!(value["summary"]["agents"]["offline"], 1);
        assert_eq!(value["summary"]["nodes"]["total"], 1);
        assert_eq!(value["summary"]["nodes"]["active"], 1);
        assert_eq!(value["summary"]["nodes"]["unknown"], 1);
        assert_eq!(value["summary"]["nodes"]["retired"], 0);
        assert_eq!(value["summary"]["networks"]["total"], 1);
        assert_eq!(value["summary"]["networks"]["with_identity_mismatch"], 0);
        let kinds = value["attention"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["kind"].as_str().unwrap().to_owned(),
                    item["severity"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert!(kinds.contains(&("agent_security_event".to_owned(), "critical".to_owned())));
        assert!(kinds.contains(&("agent_spool_fatal".to_owned(), "critical".to_owned())));
        assert!(kinds.contains(&("agent_offline".to_owned(), "warning".to_owned())));
        assert!(kinds.contains(&("agent_spool_overflow".to_owned(), "critical".to_owned())));
        assert!(kinds.contains(&("agent_report_gap".to_owned(), "warning".to_owned())));
        assert!(kinds.contains(&("node_health_unknown".to_owned(), "warning".to_owned())));
        // Critical items sort before warnings.
        assert_eq!(value["attention"][0]["severity"], "critical");
        let item = |kind: &str| {
            value["attention"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["kind"] == kind)
                .unwrap()
        };
        // The unknown Node has no observation timestamp: the Server reports
        // null instead of substituting the snapshot generation time, so the
        // WebUI cannot present a snapshot refresh as an event time.
        let unknown = item("node_health_unknown");
        assert!(unknown["observed_at"].is_null());
        assert_ne!(unknown["observed_at"], value["generated_at"]);
        // Each item carries its own evidence time, not one Agent-wide clock:
        // the gap interval uses its recorded event time, the spool items use
        // the Host observation, and the shutdown item uses its own update.
        assert_eq!(
            item("agent_report_gap")["observed_at"],
            serde_json::json!(gap_time)
        );
        assert_eq!(
            item("agent_spool_fatal")["observed_at"],
            serde_json::json!(stale)
        );
        assert_eq!(
            item("agent_spool_overflow")["observed_at"],
            serde_json::json!(stale)
        );
        assert_eq!(
            item("agent_offline")["observed_at"],
            serde_json::json!(stale)
        );
        // The accumulated security counter has no per-event timestamp.
        assert!(item("agent_security_event")["observed_at"].is_null());
    }

    #[tokio::test]
    async fn overview_network_summary_counts_distinct_eligible_networks() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let created = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-overview-networks', 1, $1, $2)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        for (network_key, display_name, genesis_hash) in [
            ("network-distinct", "Distinct", "0xexpected-distinct"),
            ("network-ineligible", "Ineligible", "0xexpected-ineligible"),
            ("network-retired", "Retired", "0xexpected-retired"),
        ] {
            sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ($1, $2, $3, 1, 1, 'lat', $4, $5)")
                .bind(network_key)
                .bind(display_name)
                .bind(genesis_hash)
                .bind(created)
                .bind(created)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        for (node_id, network_key, lifecycle, identity_state) in [
            ("node-mismatch-a", "network-distinct", "active", "ok"),
            ("node-mismatch-b", "network-distinct", "active", "ok"),
            ("node-partial", "network-ineligible", "active", "ok"),
            (
                "node-retained-error",
                "network-ineligible",
                "active",
                "error",
            ),
            ("node-retired-mismatch", "network-retired", "retired", "ok"),
        ] {
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, 'agent-overview-networks', $2, 'ws://127.0.0.1:1', $3, 'private', 1, $4, $5)")
                .bind(node_id)
                .bind(network_key)
                .bind(lifecycle)
                .bind(created)
                .bind(created)
                .execute(state.db().pool())
                .await
                .unwrap();
            sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, observation_at, state_revision, value_revision) VALUES ('agent-overview-networks', 'node', $1, $2, 'network_identity', $3, $4, $5, $6, $6, 1, 1)")
                .bind(node_id)
                .bind(node_id)
                .bind(identity_state)
                .bind(created)
                .bind(created)
                .bind(created)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        for node_id in ["node-mismatch-a", "node-mismatch-b"] {
            sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, updated_at) VALUES ($1, '0xexpected-distinct', 999, 1, 'lat', $2)")
                .bind(node_id)
                .bind(created)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_chain_id, updated_at) VALUES ('node-partial', 999, $1)")
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, updated_at) VALUES ('node-retained-error', '0xexpected-ineligible', 999, 1, 'lat', $1)")
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, updated_at) VALUES ('node-retired-mismatch', '0xexpected-retired', 999, 1, 'lat', $1)")
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = overview(
            State(state),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let summary = &value["summary"];
        assert_eq!(summary["networks"]["total"], 3);
        assert_eq!(summary["networks"]["with_identity_mismatch"], 1);
        assert_eq!(summary["nodes"]["total"], 5);
        assert_eq!(summary["nodes"]["active"], 4);
        assert_eq!(summary["nodes"]["retired"], 1);

        let count = |path: &[&str]| {
            path.iter()
                .fold(summary, |value, key| &value[*key])
                .as_i64()
                .unwrap()
        };
        assert_eq!(
            count(&["agents", "total"]),
            count(&["agents", "online"])
                + count(&["agents", "offline"])
                + count(&["agents", "unknown"])
        );
        assert_eq!(
            count(&["nodes", "active"]),
            count(&["nodes", "healthy"])
                + count(&["nodes", "unhealthy"])
                + count(&["nodes", "unknown"])
        );
        assert_eq!(
            count(&["nodes", "total"]),
            count(&["nodes", "active"]) + count(&["nodes", "retired"])
        );
        assert!(count(&["networks", "with_identity_mismatch"]) <= count(&["networks", "total"]));
    }

    #[tokio::test]
    async fn overview_counts_retired_nodes_outside_health_buckets() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let created = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-retired', 1, $1, $2)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', $1, $2)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-retired', 'agent-retired', 'mainnet', 'Node Retired', 'ws://127.0.0.1:3', 'retired', 'private', 1, $1, $2)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = overview(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["summary"]["nodes"]["total"], 1);
        assert_eq!(value["summary"]["nodes"]["active"], 0);
        assert_eq!(value["summary"]["nodes"]["retired"], 1);
        assert_eq!(value["summary"]["nodes"]["healthy"], 0);
        assert_eq!(value["summary"]["nodes"]["unhealthy"], 0);
        assert_eq!(value["summary"]["nodes"]["unknown"], 0);
        assert_eq!(value["summary"]["networks"]["total"], 1);
        assert_eq!(value["summary"]["networks"]["with_identity_mismatch"], 0);
        // Retired Nodes never enter the attention queue.
        assert_eq!(value["attention"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn partial_identity_mismatch_is_not_actionable_in_overview() {
        let (_dir, state) = node_inventory_state().await;
        sqlx::query(
            "UPDATE current_node_chain_observations SET network_genesis_hash = NULL, network_chain_id = 999999, network_p2p_network_id = NULL, network_address_hrp = NULL WHERE node_id = 'node-mismatched'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        sqlx::query(
            "UPDATE component_status SET state = 'ok', value_revision = 1 WHERE node_id = 'node-mismatched' AND component_key = 'network_identity'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();

        let detail = admin_node_detail(
            State(state.clone()),
            Path("node-mismatched".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        let detail_value: Value =
            serde_json::from_slice(&to_bytes(detail.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(detail_value["identity"]["state"], "unknown");
        assert!(
            detail_value["identity"]["mismatched_fields"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let overview_response = overview(
            State(state),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        let overview_value: Value = serde_json::from_slice(
            &to_bytes(overview_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(
            !overview_value["attention"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| { item["id"] == "node_identity_mismatch:node:node-mismatched" })
        );
    }
}
