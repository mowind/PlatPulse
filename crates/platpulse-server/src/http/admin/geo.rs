//! Geo provider status, selection, and refresh.
use super::*;
use crate::http::realtime::{AdminResource, PublicResource};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct GeoProviderOption {
    pub provider: String,
    pub label: String,
    pub available: bool,
    /// Safe explanation when the option cannot be selected. It never contains
    /// a filesystem path, raw address, or provider internals.
    pub unavailable_reason: Option<String>,
    /// Whether selecting this provider sends observed Peer public addresses
    /// to a third-party service. The Settings surface reads this Server-owned
    /// flag instead of hardcoding which providers do it, so an Owner always
    /// knows the privacy boundary before selecting.
    pub sends_peer_addresses: bool,
    /// The exact consequence of selecting this provider, including its fixed
    /// destination, or null for a provider that keeps addresses on this
    /// Server. The Server owns the sentence because it owns the destination:
    /// the browser renders this string and never composes one, so the
    /// disclosure and the endpoint a request is really sent to cannot drift
    /// apart.
    pub disclosure: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct GeoStatusDiagnostic {
    /// The selected provider. Only implemented providers are ever reported.
    pub provider: String,
    pub provider_label: String,
    /// Monotonic configuration generation. Every background result carries
    /// the generation that scheduled it and is discarded if it no longer
    /// matches the selection.
    pub provider_generation: u64,
    pub providers: Vec<GeoProviderOption>,
    /// Effective database/result state: disabled, current, stale, or error.
    /// It is independent of Peer collection freshness and Node health.
    pub state: String,
    /// Whether this deployment configured a local GeoLite2 Country database.
    pub configured: bool,
    pub build_epoch: Option<u64>,
    pub digest: Option<String>,
    pub loaded_at: Option<String>,
    pub last_error: Option<String>,
    /// Distinct countries with a currently valid retained result.
    pub cache_country_count: i64,
    /// Current Peer addresses still waiting for a fresh country result. It is
    /// null while Geo is disabled, because nothing is scheduled then.
    pub pending_lookup_count: Option<i64>,
    pub last_success_at: Option<String>,
    /// The bounded backoff window an external provider currently imposes, or
    /// null when the provider may be asked again now. It is a Server-owned
    /// instant: no address, endpoint, or provider error crosses this DTO.
    pub rate_limited_until: Option<String>,
    /// The global refresh run the Settings surface reports: running, or the
    /// terminal result of the newest run, or null when no refresh has been
    /// requested since this Server process started.
    pub refresh: Option<GeoRefreshStatus>,
    /// Why the global refresh cannot start in this configuration, or null
    /// when it can. It is a stable, path-free sentence, so a Disabled
    /// provider is explained before the Owner clicks anything.
    pub refresh_unavailable_reason: Option<String>,
}

/// One Owner-triggered global refresh run (issue #136). Every count is a
/// count of address lookups. `peer_records_in_scope` keeps the Public
/// projection's per-Node Peer-record denominator beside them, so the two
/// units are never conflated.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct GeoRefreshStatus {
    /// An opaque run identifier. It carries no address.
    pub run_id: String,
    pub provider: String,
    pub provider_label: String,
    pub provider_generation: u64,
    /// running, completed, or aborted.
    pub state: String,
    /// A stable machine-readable reason when the run stopped early, or null.
    pub abort_code: Option<String>,
    /// The matching non-sensitive sentence, or null when the run is running
    /// or completed.
    pub abort_reason: Option<String>,
    /// Distinct public Peer addresses the run resolves. One lookup per
    /// address; this is not a count of Peer records.
    pub total_lookups: u64,
    /// Current Peer records that reference those addresses. One address may
    /// serve several records, so this is never derived from the lookup count.
    pub peer_records_in_scope: u64,
    /// Lookups that reached an authoritative outcome. It stays below
    /// `total_lookups` while the run is running or after it was aborted.
    pub completed_lookups: u64,
    pub resolved_lookups: u64,
    pub no_country_lookups: u64,
    pub failed_lookups: u64,
    /// Requests the provider refused while rate limiting this Server. A
    /// rate-limited request is not a result and never counts as completed.
    pub rate_limited_lookups: u64,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct GeoRefreshMutationResponse {
    pub refresh: GeoRefreshStatus,
    /// False when this request joined the identical run that was already in
    /// progress instead of scheduling a second batch.
    pub started: bool,
    pub audit_event_id: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct GeoProviderUpdateRequest {
    pub provider: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct GeoProviderMutationResponse {
    pub geo: GeoStatusDiagnostic,
    pub audit_event_id: i64,
}

/// Build the Owner-only Geo diagnostic. Every value is computed from the
/// selected provider only; a path or raw address never enters the DTO.
async fn geo_diagnostic(state: &AppState) -> Result<GeoStatusDiagnostic, sqlx::Error> {
    let config = state.geo_config();
    let status = state.geo_status();
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let rebuild_before = crate::geo::cache_rebuild_cutoff(&now);
    let provider = config.provider.as_str();
    let cache_country_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(DISTINCT country_code) FROM geo_location_cache WHERE provider = $1 AND country_code IS NOT NULL AND expires_at > $2 AND created_at > $3",
    )
    .bind(provider)
    .bind(&now)
    .bind(&rebuild_before)
    .fetch_one(state.db().pool())
    .await?;
    let last_success_at = sqlx::query_scalar::<_, Option<String>>(
        "SELECT MAX(last_success_at) FROM geo_location_cache WHERE provider = $1",
    )
    .bind(provider)
    .fetch_one(state.db().pool())
    .await?;
    // The reported backlog is the whole queue, not the bounded batch one pass
    // is allowed to schedule.
    let pending_lookup_count = if config.provider == crate::geo::GeoProvider::Disabled {
        None
    } else {
        Some(
            crate::geo_backfill::pending_addresses(state.db().pool(), config.provider, &now, None)
                .await?
                .len() as i64,
        )
    };
    // The bounded rate-limit window belongs to the selected provider's own
    // outbound path, so a backoff armed by one external provider is never
    // reported for another. Providers that resolve on this Server have no
    // outbound path and therefore no window.
    let rate_limited_until = state
        .external_geo(config.provider)
        .and_then(|client| client.throttled_until(&now));
    let providers = crate::geo::GeoProvider::ALL
        .iter()
        .map(|candidate| GeoProviderOption {
            provider: candidate.as_str().to_owned(),
            label: candidate.label().to_owned(),
            available: config.can_run(*candidate),
            unavailable_reason: config.unavailable_reason(*candidate).map(str::to_owned),
            sends_peer_addresses: candidate.sends_peer_addresses(),
            disclosure: candidate
                .external_profile()
                .map(|profile| profile.disclosure.to_owned()),
        })
        .collect();
    Ok(GeoStatusDiagnostic {
        provider: config.provider.as_str().to_owned(),
        provider_label: config.provider.label().to_owned(),
        provider_generation: config.generation,
        providers,
        state: status.state,
        configured: status.configured,
        build_epoch: status.build_epoch,
        digest: status.digest,
        loaded_at: status.loaded_at,
        last_error: status.last_error,
        cache_country_count,
        pending_lookup_count,
        last_success_at,
        rate_limited_until,
        refresh: state
            .geo_refresh()
            .snapshot()
            .map(|snapshot| refresh_dto(&snapshot)),
        refresh_unavailable_reason: crate::geo_refresh::refresh_unavailable_reason(&config)
            .map(str::to_owned),
    })
}

/// Map one run to its Admin projection. It carries counts and stable reasons
/// only: no address, provider error text, or endpoint crosses this boundary.
fn refresh_dto(snapshot: &crate::geo_refresh::GeoRefreshSnapshot) -> GeoRefreshStatus {
    GeoRefreshStatus {
        run_id: snapshot.run_id.clone(),
        provider: snapshot.provider.as_str().to_owned(),
        provider_label: snapshot.provider.label().to_owned(),
        provider_generation: snapshot.provider_generation,
        state: snapshot.state.as_str().to_owned(),
        abort_code: snapshot.abort.map(|abort| abort.code().to_owned()),
        abort_reason: snapshot.abort.map(|abort| abort.message().to_owned()),
        total_lookups: snapshot.total_lookups,
        peer_records_in_scope: snapshot.peer_records_in_scope,
        completed_lookups: snapshot.completed_lookups,
        resolved_lookups: snapshot.resolved_lookups,
        no_country_lookups: snapshot.no_country_lookups,
        failed_lookups: snapshot.failed_lookups,
        rate_limited_lookups: snapshot.rate_limited_lookups,
        started_at: snapshot.started_at.clone(),
        finished_at: snapshot.finished_at.clone(),
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/geo",
    tag = "admin",
    responses((status = 200, description = "Owner-only safe Geo provider status", body = GeoStatusDiagnostic), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_geo_status(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    match geo_diagnostic(&state).await {
        Ok(diagnostic) => Json(diagnostic).into_response(),
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}

/// Record the Audit Event for one accepted refresh request. It is written
/// before any address is resolved, so a refresh that the Server cannot record
/// is abandoned instead of running unaudited. The detail carries the run
/// identifier, the frozen scope sizes, and whether the request started the
/// run or joined an identical one.
async fn record_geo_refresh_audit(
    state: &AppState,
    principal: &crate::http::AuthenticatedSession,
    snapshot: &crate::geo_refresh::GeoRefreshSnapshot,
    started: bool,
) -> Result<i64, sqlx::Error> {
    let mut tx = crate::database::begin(state.db().pool()).await?;
    let audit_event_id = crate::auth::insert_audit_change(
        &mut *tx,
        Some(&principal.0.user_id),
        "geo_refresh_requested",
        "geo",
        &snapshot.run_id,
        None,
        Some(&serde_json::json!({
            "run_id": snapshot.run_id,
            "provider": snapshot.provider.as_str(),
            "provider_generation": snapshot.provider_generation,
            "started": started,
            "total_lookups": snapshot.total_lookups,
            "peer_records_in_scope": snapshot.peer_records_in_scope,
        })),
    )
    .await?;
    tx.commit().await?;
    Ok(audit_event_id)
}

/// Owner-only global Peer geolocation refresh (issue #136).
///
/// It re-resolves every eligible public Peer address the Server currently
/// references, bypassing the seven-day validity of the retained cache, through
/// the same provider path the automatic background pass uses. The response is
/// the run itself: the Settings surface reads real per-address progress from
/// `GET /api/admin/v1/geo` and follows it through the existing Geo
/// invalidation. A repeated request joins the identical running run instead
/// of scheduling a second batch, and Geo that cannot resolve anything is
/// refused with a stable public error instead of a fake success.
///
/// The request carries no body, so - like every other bodyless Admin mutation -
/// the guard enforces the session, the Owner role, the Origin, and the CSRF
/// token without requiring a JSON content type; there is no request document
/// a form-encoded cross-site post could smuggle in.
#[utoipa::path(
    post,
    path = "/api/admin/v1/geo/refresh",
    tag = "admin",
    responses((status = 200, description = "Owner-only Geo refresh run", body = GeoRefreshMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn trigger_geo_refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let run_id = format!("geo-refresh-{}", uuid::Uuid::new_v4());
    let start = match crate::geo_refresh::begin_refresh(&state, run_id, &now).await {
        Ok(start) => start,
        Err(crate::geo_refresh::StartRefreshError::Unavailable(reason)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::new(
                    "geo_refresh_unavailable",
                    reason,
                    &request_id.0,
                )),
            )
                .into_response();
        }
        Err(crate::geo_refresh::StartRefreshError::Database(error)) => {
            return storage_failure(&state, &request_id.0, &error);
        }
    };
    let started = matches!(start, crate::geo_refresh::RefreshStart::Started(_));
    let snapshot = match &start {
        crate::geo_refresh::RefreshStart::Started(prepared) => prepared.snapshot(),
        crate::geo_refresh::RefreshStart::Joined(snapshot) => snapshot.clone(),
    };
    let audit_event_id =
        match record_geo_refresh_audit(&state, &principal, &snapshot, started).await {
            Ok(audit_event_id) => audit_event_id,
            Err(error) => {
                // Nothing may resolve without its Audit Event. A prepared run is
                // abandoned truthfully instead of being left "running" forever.
                if let crate::geo_refresh::RefreshStart::Started(prepared) = start {
                    prepared.abandon(crate::geo_refresh::RefreshAbort::InternalError, &now);
                }
                return storage_failure(&state, &request_id.0, &error);
            }
        };
    if let crate::geo_refresh::RefreshStart::Started(prepared) = start {
        crate::geo_refresh::spawn_refresh(&state, prepared);
    }
    Json(GeoRefreshMutationResponse {
        refresh: refresh_dto(&snapshot),
        started,
        audit_event_id,
    })
    .into_response()
}

/// Select the Geo provider. The change is Owner-only, Origin/JSON/CSRF
/// guarded, audited, durably persisted with a new configuration generation,
/// and applied to the running process in the same breath, so scheduled work
/// from the previous selection can never be written as the current one.
#[utoipa::path(
    put,
    path = "/api/admin/v1/geo/provider",
    tag = "admin",
    request_body = GeoProviderUpdateRequest,
    responses((status = 200, description = "Owner-only Geo provider change", body = GeoProviderMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn update_geo_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    Json(request): Json<GeoProviderUpdateRequest>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    let Some(provider) = crate::geo::GeoProvider::parse(&request.provider) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_fields(
                "unknown_geo_provider",
                "unknown Geo provider",
                &request_id.0,
                vec!["provider".to_owned()],
            )),
        )
            .into_response();
    };
    let config = state.geo_config();
    if let Some(reason) = config.unavailable_reason(provider) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_fields(
                "geo_provider_unavailable",
                reason,
                &request_id.0,
                vec!["provider".to_owned()],
            )),
        )
            .into_response();
    }

    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let mut tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    // The generation is advanced inside this transaction and read back from
    // the same statement, so two concurrent Owner changes can never persist
    // the same generation.
    let generation = match crate::geo::bump_provider_generation(&mut *tx, &now).await {
        Ok(generation) => generation,
        Err(error) => {
            let _ = tx.rollback().await;
            return storage_failure(&state, &request_id.0, &error);
        }
    };
    let selection = crate::geo::GeoSelection {
        provider,
        generation,
    };
    if let Err(error) = crate::geo::write_provider_selection(&mut *tx, selection, &now).await {
        let _ = tx.rollback().await;
        return storage_failure(&state, &request_id.0, &error);
    }
    let audit_event_id = match crate::auth::insert_audit_change(
        &mut *tx,
        Some(&principal.0.user_id),
        "geo_provider_changed",
        "geo",
        "global",
        Some(&serde_json::json!({ "provider": config.provider.as_str() })),
        Some(&serde_json::json!({ "provider": provider.as_str() })),
    )
    .await
    {
        Ok(id) => id,
        Err(error) => {
            let _ = tx.rollback().await;
            return storage_failure(&state, &request_id.0, &error);
        }
    };
    if let Err(error) = tx.commit().await {
        return storage_failure(&state, &request_id.0, &error);
    }
    state.apply_geo_provider(crate::geo::GeoSelection {
        provider,
        generation,
    });
    // A provider that is not selected anymore keeps no raw-IP results: drop
    // them now instead of waiting for the maintenance tick.
    if let Err(error) = crate::geo::cleanup_cache(state.db().pool(), &now, provider).await {
        state.note_database_error(&error);
        tracing::error!(error = %error, "admin geo cache cleanup failed after provider change");
    }
    state
        .admin_realtime()
        .publish(AdminResource::Geo, None::<String>);
    state
        .public_realtime()
        .publish(PublicResource::Geo, None::<String>);

    match geo_diagnostic(&state).await {
        Ok(diagnostic) => Json(GeoProviderMutationResponse {
            geo: diagnostic,
            audit_event_id,
        })
        .into_response(),
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AuthenticatedSession;
    use axum::body::to_bytes;
    use serde_json::Value;
    use tempfile::tempdir;
    use time::OffsetDateTime;

    #[tokio::test]
    async fn admin_geo_status_exposes_safe_metadata_only() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        let expires_at = crate::geo::cache_expiry(&now);
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('disabled', '8.8.8.8', 'US', 'current', $1, $2, $3, $4, $5)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(expires_at)
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
        let response = admin_geo_status(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["state"], "disabled");
        assert_eq!(value["configured"], false);
        assert_eq!(value["cache_country_count"], 1);
        assert!(!text.contains("8.8.8.8"));
        assert!(!text.contains("server.db"));
    }

    /// Build a Geo-capable Admin state. `with_database` copies the real
    /// GeoIP2 Country test fixture into the state directory with private
    /// permissions and loads it, so the mutation path is exercised against a
    /// real MMDB rather than a stub.
    async fn geo_admin_state(dir: &tempfile::TempDir, with_database: bool) -> AppState {
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        if !with_database {
            return AppState::new(database, None, auth);
        }
        let path = dir.path().join("GeoIP2-Country-Test.mmdb");
        crate::geo::write_test_database(&path);
        let loader = std::sync::Arc::new(crate::geo::GeoLoader::new(Some(path)));
        assert!(loader.reload());
        let state = AppState::new(database, None, auth).with_geo_loader(loader);
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        state
    }

    fn geo_owner_session() -> AuthenticatedSession {
        AuthenticatedSession(crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        })
    }

    fn geo_mutation_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(header::ORIGIN, "http://127.0.0.1:8080".parse().unwrap());
        headers.insert("x-csrf-token", "csrf".parse().unwrap());
        headers
    }

    fn geo_request_id() -> crate::http::RequestId {
        crate::http::RequestId(std::sync::Arc::from("req-geo-1"))
    }

    async fn geo_body(response: Response) -> Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn geo_provider_selection_is_guarded_persisted_and_audited() {
        let dir = tempdir().unwrap();
        let state = geo_admin_state(&dir, true).await;
        let session = geo_owner_session();

        let response = admin_geo_status(
            State(state.clone()),
            Extension(session.clone()),
            Extension(geo_request_id()),
        )
        .await;
        let body = geo_body(response).await;
        assert_eq!(body["provider"], "disabled");
        assert_eq!(body["provider_label"], "Disabled");
        assert_eq!(body["provider_generation"], 0);
        assert_eq!(body["providers"][0]["provider"], "disabled");
        assert_eq!(body["providers"][0]["available"], true);
        assert_eq!(body["providers"][0]["sends_peer_addresses"], false);
        assert_eq!(body["providers"][1]["provider"], "local_mmdb");
        assert_eq!(body["providers"][1]["available"], true);
        assert_eq!(body["providers"][1]["sends_peer_addresses"], false);
        // IPinfo is offered because it is really implemented, and the
        // Server states the outbound consequence before the Owner selects it.
        assert_eq!(body["providers"][2]["provider"], "ipinfo");
        assert_eq!(body["providers"][2]["label"], "IPinfo");
        assert_eq!(body["providers"][2]["available"], true);
        assert_eq!(body["providers"][2]["sends_peer_addresses"], true);
        // GeoJS is offered on exactly the same terms: it is implemented, it
        // needs no local database, and the Server states the outbound
        // consequence before the Owner selects it (issue #135).
        assert_eq!(body["providers"][3]["provider"], "geojs");
        assert_eq!(body["providers"][3]["label"], "GeoJS");
        assert_eq!(body["providers"][3]["available"], true);
        assert_eq!(body["providers"][3]["sends_peer_addresses"], true);
        // Exactly the four implemented options are offered, in one stable
        // order; nothing else became selectable.
        assert_eq!(body["providers"].as_array().unwrap().len(), 4);
        assert!(body["rate_limited_until"].is_null());
        assert!(body["pending_lookup_count"].is_null());
        // No unimplemented provider or arbitrary endpoint is ever offered.
        let text = body.to_string();
        for absent in ["ip-api", "ipapi", "maxmind.com/download", "http://"] {
            assert!(!text.contains(absent), "{absent} must not be selectable");
        }

        // CSRF/Origin/JSON guard.
        let response = update_geo_provider(
            State(state.clone()),
            HeaderMap::new(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "local_mmdb".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // Unknown providers are rejected instead of silently ignored.
        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "ip-api".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            geo_body(response).await["error"]["code"],
            "unknown_geo_provider"
        );

        // A retained result of another provider is not current data and is
        // dropped when the selection changes, even while the Peer reference
        // that justified retaining it still exists.
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, last_received_at, created_at, updated_at) VALUES ('agent-geo-admin', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('geo-net', 'Geo Network', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('geo-admin-node', 'agent-geo-admin', 'geo-net', 'Geo Node', 'ws://127.0.0.1:1', 'active', 'public', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES ('geo-admin-node', 'peer-1', '8.8.8.8', 'inbound', 0, 0, 0, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        for provider in ["disabled", "local_mmdb"] {
            sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ($1, '8.8.8.8', 'US', 'current', $2, $3, $4, $5, $6)")
                .bind(provider)
                .bind(&now)
                .bind(&now)
                .bind(&now)
                .bind(&now)
                .bind(crate::geo::cache_expiry(&now))
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "local_mmdb".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = geo_body(response).await;
        assert_eq!(body["geo"]["provider"], "local_mmdb");
        assert_eq!(body["geo"]["provider_generation"], 1);
        assert!(body["audit_event_id"].as_i64().unwrap() > 0);
        assert_eq!(
            state.geo_config().provider,
            crate::geo::GeoProvider::LocalMmdb
        );
        assert_eq!(state.geo_config().generation, 1);
        let stored: String = sqlx::query_scalar(
            "SELECT setting_value FROM server_settings WHERE setting_key = 'geo_provider'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, "local_mmdb");
        let stored_generation: String = sqlx::query_scalar(
            "SELECT setting_value FROM server_settings WHERE setting_key = 'geo_provider_generation'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored_generation, "1");
        let audit_kind: String = sqlx::query_scalar(
            "SELECT event_kind FROM audit_events WHERE event_kind = 'geo_provider_changed'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit_kind, "geo_provider_changed");
        let providers_left: Vec<String> =
            sqlx::query_scalar("SELECT provider FROM geo_location_cache ORDER BY provider")
                .fetch_all(state.db().pool())
                .await
                .unwrap();
        assert_eq!(providers_left, vec!["local_mmdb".to_owned()]);

        // Repeating the same selection still advances the generation, so work
        // scheduled before the change can never be written as current.
        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "local_mmdb".to_owned(),
            }),
        )
        .await;
        assert_eq!(geo_body(response).await["geo"]["provider_generation"], 2);

        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "disabled".to_owned(),
            }),
        )
        .await;
        let body = geo_body(response).await;
        assert_eq!(body["geo"]["provider"], "disabled");
        assert_eq!(body["geo"]["provider_generation"], 3);
        assert_eq!(body["geo"]["state"], "disabled");
        assert!(body["geo"]["pending_lookup_count"].is_null());
        assert_eq!(
            state.geo_config().provider,
            crate::geo::GeoProvider::Disabled
        );

        // Selecting the external provider persists, audits, and reports a
        // provider state that carries no database metadata at all. The
        // backlog is real because the peer reference is still current.
        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "ipinfo".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = geo_body(response).await;
        assert_eq!(body["geo"]["provider"], "ipinfo");
        assert_eq!(body["geo"]["provider_label"], "IPinfo");
        assert_eq!(body["geo"]["provider_generation"], 4);
        assert_eq!(body["geo"]["state"], "current");
        assert!(body["geo"]["build_epoch"].is_null());
        assert!(body["geo"]["digest"].is_null());
        assert!(body["geo"]["loaded_at"].is_null());
        assert!(body["geo"]["last_error"].is_null());
        assert!(body["geo"]["rate_limited_until"].is_null());
        assert_eq!(body["geo"]["pending_lookup_count"], 1);
        assert_eq!(state.geo_config().provider, crate::geo::GeoProvider::Ipinfo);
        let stored: String = sqlx::query_scalar(
            "SELECT setting_value FROM server_settings WHERE setting_key = 'geo_provider'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, "ipinfo");
        // The local results of the previous selection can never be read as
        // the external provider's data, so they are already gone.
        let providers_left: Vec<String> =
            sqlx::query_scalar("SELECT provider FROM geo_location_cache ORDER BY provider")
                .fetch_all(state.db().pool())
                .await
                .unwrap();
        assert!(providers_left.is_empty());

        // Switching between the two External Geo Providers keeps the same
        // contract: the new provider persists and audits, its diagnostic
        // carries no database metadata, and no row retained for the provider
        // that was just retired can be read as the new one's result (#135).
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('ipinfo', '8.8.8.8', 'US', 'current', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "geojs".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = geo_body(response).await;
        assert_eq!(body["geo"]["provider"], "geojs");
        assert_eq!(body["geo"]["provider_label"], "GeoJS");
        assert_eq!(body["geo"]["provider_generation"], 5);
        assert_eq!(body["geo"]["state"], "current");
        assert!(body["geo"]["build_epoch"].is_null());
        assert!(body["geo"]["digest"].is_null());
        assert!(body["geo"]["loaded_at"].is_null());
        assert!(body["geo"]["last_error"].is_null());
        assert!(body["geo"]["rate_limited_until"].is_null());
        // The backlog is counted against the newly selected provider only.
        assert_eq!(body["geo"]["pending_lookup_count"], 1);
        assert_eq!(state.geo_config().provider, crate::geo::GeoProvider::GeoJs);
        let stored: String = sqlx::query_scalar(
            "SELECT setting_value FROM server_settings WHERE setting_key = 'geo_provider'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, "geojs");
        let providers_left: Vec<String> =
            sqlx::query_scalar("SELECT provider FROM geo_location_cache ORDER BY provider")
                .fetch_all(state.db().pool())
                .await
                .unwrap();
        assert!(
            providers_left.is_empty(),
            "the retired provider's raw-IP rows are dropped immediately"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'geo_provider_changed'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            5,
            "every provider change including the GeoJS one is audited"
        );

        // Disabling an external provider stops scheduling entirely; it never
        // falls back to the other external provider.
        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "disabled".to_owned(),
            }),
        )
        .await;
        let body = geo_body(response).await;
        assert_eq!(body["geo"]["provider_generation"], 6);
        assert_eq!(body["geo"]["state"], "disabled");
        assert!(body["geo"]["pending_lookup_count"].is_null());
    }

    /// A repeated Owner request joins the run that is already in progress
    /// through the real endpoint: it returns the same run identifier, reports
    /// `started: false`, and writes a second Audit Event for the request
    /// without scheduling a second batch.
    #[tokio::test]
    async fn repeated_geo_refresh_requests_join_the_same_run() {
        let dir = tempdir().unwrap();
        let state = geo_admin_state(&dir, true).await;
        let session = geo_owner_session();
        update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "local_mmdb".to_owned(),
            }),
        )
        .await;
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, last_received_at, created_at, updated_at) VALUES ('agent-geo-refresh', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('geo-net', 'Geo Network', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('geo-refresh-node', 'agent-geo-refresh', 'geo-net', 'Geo Node', 'ws://127.0.0.1:1', 'active', 'public', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES ('geo-refresh-node', 'peer-1', '89.160.20.112', 'inbound', 0, 0, 0, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();

        let first = trigger_geo_refresh(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let first = geo_body(first).await;
        assert_eq!(first["started"], true);

        // The second request is the same Owner clicking again. It must not
        // create a competing batch: it reports the identical run identifier
        // and `started: false`. The request is still audited.
        let second = trigger_geo_refresh(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
        )
        .await;
        assert_eq!(second.status(), StatusCode::OK);
        let second = geo_body(second).await;
        assert_eq!(second["started"], false);
        assert_eq!(second["refresh"]["run_id"], first["refresh"]["run_id"]);
        assert_eq!(second["refresh"]["total_lookups"], 1);
        let requested: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'geo_refresh_requested'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(requested, 2, "every accepted request is audited");

        // Exactly one lookup happened for the one address, whatever the
        // number of Owner clicks.
        let terminal = wait_for_geo_refresh(&state).await;
        assert_eq!(terminal["refresh"]["state"], "completed");
        assert_eq!(terminal["refresh"]["completed_lookups"], 1);
        assert_eq!(terminal["refresh"]["total_lookups"], 1);
    }

    /// Disabling Geo while a run is in flight aborts it, and the Settings
    /// diagnostic reports the truthful terminal state instead of a run that
    /// stays "running" forever or claims results of a configuration that is
    /// gone.
    #[tokio::test]
    async fn disabling_geo_during_a_run_aborts_the_diagnostic_truthfully() {
        let dir = tempdir().unwrap();
        let state = geo_admin_state(&dir, true).await;
        let session = geo_owner_session();
        update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "local_mmdb".to_owned(),
            }),
        )
        .await;
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, last_received_at, created_at, updated_at) VALUES ('agent-geo-refresh', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('geo-net', 'Geo Network', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('geo-refresh-node', 'agent-geo-refresh', 'geo-net', 'Geo Node', 'ws://127.0.0.1:1', 'active', 'public', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES ('geo-refresh-node', 'peer-1', '89.160.20.112', 'inbound', 0, 0, 0, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();

        // Prepare the run exactly as the endpoint does before it records the
        // Audit Event, then disable Geo while it is the registry's running run.
        let prepared = match crate::geo_refresh::begin_refresh(
            &state,
            "run-switch".to_owned(),
            &crate::auth::format_rfc3339(crate::auth::now_utc()),
        )
        .await
        .unwrap()
        {
            crate::geo_refresh::RefreshStart::Started(prepared) => prepared,
            crate::geo_refresh::RefreshStart::Joined(_) => panic!("a clean state starts a run"),
        };
        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "disabled".to_owned(),
            }),
        )
        .await;
        let body = geo_body(response).await;
        assert_eq!(body["geo"]["provider"], "disabled");
        let refresh = &body["geo"]["refresh"];
        assert_eq!(refresh["state"], "aborted");
        assert_eq!(refresh["abort_code"], "provider_changed");
        assert_eq!(refresh["provider"], "local_mmdb", "the run's own provider");
        assert!(
            refresh["abort_reason"]
                .as_str()
                .unwrap()
                .contains("provider or its configuration changed")
        );
        assert_eq!(refresh["completed_lookups"], 0);
        assert!(refresh["finished_at"].is_string());
        assert!(body["geo"]["refresh_unavailable_reason"].is_string());
        prepared.abandon(
            crate::geo_refresh::RefreshAbort::Superseded,
            "2026-01-01T00:00:00Z",
        );
    }

    /// Read the Settings diagnostic until the refresh run reaches a terminal
    /// state, then return that body. It waits on the observable REST state
    /// instead of sleeping for a fixed duration.
    async fn wait_for_geo_refresh(state: &AppState) -> Value {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let response = admin_geo_status(
                State(state.clone()),
                Extension(geo_owner_session()),
                Extension(geo_request_id()),
            )
            .await;
            let body = geo_body(response).await;
            if body["refresh"]["state"] != "running" {
                return body;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the refresh run reaches an observable terminal state"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// The Owner-only global refresh (issue #136): it really re-resolves the
    /// current scope through the loaded MMDB, reports real per-address
    /// progress, is audited, is refused for Disabled Geo instead of faking a
    /// success, and never exposes a raw address.
    #[tokio::test]
    async fn geo_refresh_reports_real_progress_and_refuses_disabled_geo() {
        let dir = tempdir().unwrap();
        let state = geo_admin_state(&dir, true).await;
        let session = geo_owner_session();

        // Geo is Disabled, so no address can be resolved: the request is
        // refused with a stable public error rather than reported as a run.
        let response = trigger_geo_refresh(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = geo_body(response).await;
        assert_eq!(body["error"]["code"], "geo_refresh_unavailable");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Disabled")
        );
        assert!(state.geo_refresh().snapshot().is_none());

        // Origin/JSON/CSRF are checked before anything is scheduled.
        let response = trigger_geo_refresh(
            State(state.clone()),
            HeaderMap::new(),
            Extension(session.clone()),
            Extension(geo_request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(state.geo_refresh().snapshot().is_none());

        // Select Local MMDB and give the Server one current Peer reference.
        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "local_mmdb".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, last_received_at, created_at, updated_at) VALUES ('agent-geo-refresh', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('geo-net', 'Geo Network', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('geo-refresh-node', 'agent-geo-refresh', 'geo-net', 'Geo Node', 'ws://127.0.0.1:1', 'active', 'public', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES ('geo-refresh-node', 'peer-1', '89.160.20.112', 'inbound', 0, 0, 0, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = trigger_geo_refresh(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session.clone()),
            Extension(geo_request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = geo_body(response).await;
        assert_eq!(body["started"], true);
        assert!(body["audit_event_id"].as_i64().unwrap() > 0);
        assert_eq!(body["refresh"]["provider"], "local_mmdb");
        assert_eq!(body["refresh"]["total_lookups"], 1);
        assert_eq!(body["refresh"]["peer_records_in_scope"], 1);
        assert!(
            !body.to_string().contains("89.160.20.112"),
            "the Admin refresh DTO never carries a raw address"
        );

        // The terminal result is read from the same GET the Settings surface
        // polls, and the real cache row proves the lookup happened.
        let terminal = wait_for_geo_refresh(&state).await;
        assert_eq!(terminal["refresh"]["state"], "completed");
        assert_eq!(terminal["refresh"]["completed_lookups"], 1);
        assert_eq!(terminal["refresh"]["resolved_lookups"], 1);
        assert_eq!(terminal["refresh"]["no_country_lookups"], 0);
        assert_eq!(terminal["refresh"]["failed_lookups"], 0);
        assert!(terminal["refresh"]["finished_at"].is_string());
        let country: Option<String> = sqlx::query_scalar(
            "SELECT country_code FROM geo_location_cache WHERE provider = 'local_mmdb' AND canonical_ip = '89.160.20.112'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(country.as_deref(), Some("SE"));

        // The request is audited with its frozen scope, and the audit detail
        // never carries the address either.
        let audit: (i64, Option<String>) = sqlx::query_as(
            "SELECT COUNT(*), MAX(after_json) FROM audit_events WHERE event_kind = 'geo_refresh_requested'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit.0, 1);
        let detail = audit.1.unwrap();
        assert!(detail.contains("total_lookups"));
        assert!(!detail.contains("89.160.20.112"));
    }

    #[tokio::test]
    async fn a_configured_but_unusable_database_is_visible_before_it_is_selected() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        // A configured file that cannot be parsed as an MMDB.
        let path = dir.path().join("broken.mmdb");
        std::fs::write(&path, b"not an MMDB").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let loader = std::sync::Arc::new(crate::geo::GeoLoader::new(Some(path)));
        assert!(!loader.reload());
        let state = AppState::new(database, None, auth).with_geo_loader(loader);

        let response = admin_geo_status(
            State(state.clone()),
            Extension(geo_owner_session()),
            Extension(geo_request_id()),
        )
        .await;
        let body = geo_body(response).await;
        // Disabled is still the selected provider, but the Owner can see that
        // the database Local MMDB would use is unusable before choosing it.
        assert_eq!(body["provider"], "disabled");
        assert_eq!(body["state"], "disabled");
        assert_eq!(body["configured"], true);
        assert_eq!(body["providers"][1]["available"], true);
        let error = body["last_error"].as_str().unwrap();
        assert!(!error.is_empty());
        let text = body.to_string();
        assert!(!text.contains("broken.mmdb"), "no filesystem path leaks");
        assert!(
            !text.contains(dir.path().to_str().unwrap()),
            "no state directory leaks"
        );
        // The only URLs this diagnostic may carry are the fixed destinations
        // the Server-owned disclosures name for the providers it implements.
        // Everything else (a database path, a request URL, provider error
        // text) must stay out.
        for after_scheme in text.split("https://").skip(1) {
            let host = after_scheme.split(['/', '"']).next().unwrap_or_default();
            assert!(
                matches!(host, "ipinfo.io" | "get.geojs.io"),
                "unexpected URL in the Geo diagnostic: {host}"
            );
        }
    }

    #[tokio::test]
    async fn local_mmdb_requires_a_configured_database() {
        let dir = tempdir().unwrap();
        let state = geo_admin_state(&dir, false).await;
        let session = geo_owner_session();

        let response = admin_geo_status(
            State(state.clone()),
            Extension(session.clone()),
            Extension(geo_request_id()),
        )
        .await;
        let body = geo_body(response).await;
        assert_eq!(body["providers"][1]["provider"], "local_mmdb");
        assert_eq!(body["providers"][1]["available"], false);
        assert!(body["providers"][1]["unavailable_reason"].is_string());

        let response = update_geo_provider(
            State(state.clone()),
            geo_mutation_headers(),
            Extension(session),
            Extension(geo_request_id()),
            Json(GeoProviderUpdateRequest {
                provider: "local_mmdb".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = geo_body(response).await;
        assert_eq!(body["error"]["code"], "geo_provider_unavailable");
        assert_eq!(body["error"]["fields"][0], "provider");
        assert_eq!(
            state.geo_config().provider,
            crate::geo::GeoProvider::Disabled,
            "a rejected selection never changes the running configuration"
        );
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT setting_value FROM server_settings WHERE setting_key = 'geo_provider'",
        )
        .fetch_optional(state.db().pool())
        .await
        .unwrap();
        assert!(stored.is_none());
    }
}
