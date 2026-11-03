# Issue #208 — pre-implementation research findings

## SERVER (already essentially complete → ticket is mainly Web routing + acceptance)
crates/platpulse-server/src/http/operations_admin.rs (2995 lines), router() ~line 1882:
- GET /operations, GET /operations/{operation_id}, POST /operations/{operation_id}/cancel
- GET/PUT /history-window, POST /history-window/impact
- GET /retention, POST /retention/impact, PUT /retention/policies/{family}, POST /retention/run
- GET /backups, GET /backups/{artifact_id}, POST /backups/{artifact_id}/verify
- POST /restore/validate, POST /restore
- GET /doctor, POST /doctor

DTOs (serde camelCase + ToSchema):
- OperationSummary{operationId,kind,status,progressPercent,progressLabel,requestId,createdAt,startedAt,finishedAt,auditEventId,cancelRequested} (line 26)
- OperationIssue{code,message} (42); OperationDetail{operation,warnings,errors,result,cancellable} (49; cancellable = matches!(status,"queued"|"running")); OperationMutationResponse{operation,auditEventId} (60)
- DoctorCheckDto{checkId,label,status,detail} (518); DoctorOverview{last_run: Option<OperationSummary>, checks} (527)
- BackupArtifactSummary / BackupArtifactDetail / RestoreValidation

Semantics:
- OPERATION_STATUSES = queued,running,succeeded,succeeded_with_warnings,failed,cancelled
- OPERATION_KINDS = retention_run,backup_create,backup_verify,doctor_run,restore (backup_create retained for historical rows per ADR 0008)
- bad filter → 400 invalid_query (fields ["status"]/["kind"]); unknown op → 404 operation_not_found (fields ["operationId"])
- cancel non-queued/running or already requested → 409 operation_not_cancellable "only queued or running Operations can be cancelled" (fields ["operationId","status"])
- load_operation_detail redacts warnings/errors (crate::redaction::redact_sensitive) and result (redact_json_value)
- cancel_operation: queued → immediately STATUS_CANCELLED + finished_at; running → stays RUNNING, cancel_requested=1, finished_at NULL, audits "operation_cancelled", publishes realtime "operations". (UI must render "cancel requested" ≠ complete)
- doctor_run: mutation_guard(...) then queue_operation(kind=KIND_DOCTOR_RUN, params {}, audit "doctor_started"); no delete confirmation
- doctor_overview: newest doctor_run WITH result_json NOT NULL; in-flight run NOT surfaced (gap candidate: add current_run)

crates/platpulse-server/src/operations.rs: STATUS_* consts lines 26-31; KIND_DOCTOR_RUN line 36; create_operation, next_queued, mark_running, operation_kind, operation_params, set_progress, add_warning, add_error, is_cancel_requested, finalize, requeue_interrupted_operations (leftover running → failed, error code "interrupted_by_restart"), process_operations dispatches KIND_DOCTOR_RUN => crate::doctor::run, unknown kind → add_error "unknown_operation_kind" + FAILED.

crates/platpulse-server/src/doctor.rs: STATUS_PASS/WARNING/FAIL/NOT_CONFIGURED/SKIPPED (lines 29-33). run(): error → add_error "doctor_run_failed" + FAILED (preserves previous result); cancel_requested → CANCELLED; else SUCCEEDED_WITH_WARNINGS if any check != pass else SUCCEEDED; result={"checks":[...]}. last_run() = newest doctor_run WITH result_json NOT NULL. check_ids: database_integrity, schema_version, critical_workers, web_assets, retention_policies, backup_storage, latest_backup, backup_age, backup_residue, notification_channels, database_storage, geo_database, platform_security.

Existing tests in operations_admin.rs (tempdir + AppState harness, helpers test_state/body_json/seed_old_data): retention_overview_lists_seeded_policies_with_bounds, policy_update_within_bounds_is_audited_and_out_of_bounds_is_rejected, history_window_is_bounded_confirmed_and_audited, retention_run_deletes_only_old_rows_and_preserves_protected_state, offline_created_backup_exposes_sanitized_metadata_and_verifies, offline_backup_requires_a_configured_directory, doctor_reports_backup_age_and_partial_residue_without_touching_readiness, doctor_run_reports_distinct_statuses_without_mutating, operations_list_filters_and_cancel_terminal_conflicts, queued_operation_cancels_immediately_and_is_audited, coverage_state_stays_inspectable_while_a_run_is_running, retention_run_slims_old_receipt_bodies_without_touching_identity_or_recent_rows, running_backup_and_doctor_cancellations_are_honoured, running_operation_cancel_flag_stops_the_next_batch, restore_validate_and_submit_refuse_while_the_server_runs.

## WEB
- platpulse-web/src/pages/AdminNotifications.tsx (130 lines) = model domain overview page: useAuth().generation + useAdminX; Alert/AlertDescription/AlertTitle; CardX size="medium" className={CARD_SURFACE}; loading / isError+refetch / isRefetchError / Empty; DetailList/DetailItem.
- platpulse-web/src/pages/notificationShared.tsx (257 lines): CARD_SURFACE, shortId, errorMessage(error, fallback), indeterminateOutcome (AdminApiError code 'network_unavailable'), INDETERMINATE_OUTCOME, deliveryStateTone, deliveryIsRetryable, FormFeedbackNote, DetailList, DetailItem, NotificationSectionNav, NotificationRequestPanel.
- platpulse-web/src/layouts/AdminLayout.tsx: group "Operations" (line 210-212) = Overview/Agents/Nodes/Networks/Settings; group "Alerts" = Incidents/Rules/Silences/Maintenance/Notifications; then "Access". NavLink + NAV_LINK/NAV_LINK_ACTIVE className fn + data-slot="admin-nav-icon" lucide icon.
  DECISION: first group already named "Operations" → rename it (e.g. "Monitoring") and add a new "Operations" group with Tasks (/admin/operations) and Doctor (/admin/operations/doctor).
- platpulse-web/src/api/admin.ts (2681 lines) already imports generated ops/doctor/retention/backup SDK fns+types. Helpers: operationStatusLabel, operationKindLabel (retention_run->'Retention run', backup_create->'Backup creation', backup_verify->'Backup verification', doctor_run->'Doctor', restore->'Restore'), fetchAdminOperations(filters, signal), useAdminOperations(generation, filters), fetchAdminOperation/useAdminOperation(generation, operationId), cancelOperationEntry(operationId, csrfToken) invalidating adminKeys.all on success AND failure, useAdminBackups, doctor query ~2587, doctorRun ~2605. Transport: requestGenerated/requestAdmin + AdminApiError; adminKeys.*.
- platpulse-web/src/api/generated/ = types.gen.ts (5500), sdk.gen.ts (891), client.gen.ts, index.ts — regenerate via package.json script, verify no diff.
- No AdminOperations page exists. e2e precedents: notification-acceptance.spec.ts, rule-management-acceptance.spec.ts, silence-maintenance-acceptance.spec.ts, incident-acknowledgment-acceptance.spec.ts, server-harness.ts (525 lines).

## DOCS
- docs/design/webui.md §4.3 "Admin pages" holds authoritative Page ID -> Route -> Purpose -> Actors table (add PAGE-ADMIN-OPERATIONS / PAGE-ADMIN-OPERATION-DETAIL / PAGE-ADMIN-DOCTOR) + the paragraph after the table listing unrouted Server surfaces (line 192); lines 13 and 38 enumerate routed vs unrouted. §15.6-15.10 are per-issue "delivered" sections (15.6 #203, 15.7 #204, 15.8 #205, 15.9 #206, 15.10 #207) → add §15.11 for #208.
- docs/design/platpulse.md lines 387, 464, 527, 599 mention Operations/Doctor surfaces and SPA having no routes.

## WORKFLOW
- Labels: ready-for-agent → implemented on close (closed issue like #207 carries only implemented + CLOSED). On close: add implemented, remove ready-for-agent, comment, close.
- Frontier after #208: #209 backups, #210 retention policy/preview, #211 retention execution, #212-#217 history, #218/#219 validators, #220/#221 investigation, #222-#225 Home. Parent #202 stays open.
- Commit style (de2cf80): conventional commit "feat(server,web): <summary> (#206)" + structured body Server/Web/Docs/Gates bullets.
- Reviewer route: provider cliproxyapi, model gpt-6.1-sol, reasoning_effort high.
