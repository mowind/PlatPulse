//! Read-only Doctor diagnostics (issue #50, design §20.3, webui.md §8.4).
//!
//! Doctor never auto-fixes, deletes, migrates, or rotates secrets. It runs
//! bounded read-only checks against live state and reports each one as
//! Pass, Warning, Fail, NotConfigured, or Skipped with sanitized detail.
//! The result is stored on a `doctor_run` Operation so the previous
//! diagnostic result survives a failed run.

use serde_json::Value;

use crate::http::AppState;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    pub check_id: String,
    pub label: String,
    pub status: &'static str,
    pub detail: String,
}

pub const STATUS_PASS: &str = "pass";
pub const STATUS_WARNING: &str = "warning";
pub const STATUS_FAIL: &str = "fail";
pub const STATUS_NOT_CONFIGURED: &str = "not_configured";
pub const STATUS_SKIPPED: &str = "skipped";

/// Run every Doctor check and persist the report on the `doctor_run`
/// Operation. The Operation succeeds when all checks pass and reports
/// `SucceededWithWarnings` when any check is not Pass — never a plain
/// Success for a system with warnings. A run error fails the Operation and
/// preserves the previous diagnostic result.
pub async fn run(
    state: &AppState,
    operation_id: &str,
) -> Result<(), crate::operations::OperationError> {
    let checks = match collect_checks(state).await {
        Ok(checks) => checks,
        Err(error) => {
            let _ = crate::operations::add_error(
                state,
                operation_id,
                "doctor_run_failed",
                &crate::redaction::redact_sensitive(&error.to_string()),
            )
            .await;
            let _ = crate::operations::finalize(
                state,
                operation_id,
                crate::operations::STATUS_FAILED,
                None,
                &["doctor"],
            )
            .await;
            return Ok(());
        }
    };
    if crate::operations::is_cancel_requested(state, operation_id).await? {
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_CANCELLED,
            None,
            &["doctor"],
        )
        .await?;
        return Ok(());
    }
    let any_non_pass = checks.iter().any(|check| check.status != STATUS_PASS);
    let status = if any_non_pass {
        crate::operations::STATUS_SUCCEEDED_WITH_WARNINGS
    } else {
        crate::operations::STATUS_SUCCEEDED
    };
    let result = serde_json::json!({ "checks": checks });
    crate::operations::finalize(state, operation_id, status, Some(&result), &["doctor"]).await?;
    Ok(())
}

/// The most recent `doctor_run` Operation row that produced a diagnostic
/// result: the previous diagnostic result stays available after a failed
/// run (a failed run has no result and must never hide the last report).
pub async fn last_run(
    state: &AppState,
) -> Result<Option<(String, String, Option<String>)>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT operation_id, status, result_json FROM operations WHERE kind = 'doctor_run' AND result_json IS NOT NULL ORDER BY created_at DESC, operation_id DESC LIMIT 1",
    )
    .fetch_optional(state.db().pool())
    .await?;
    Ok(row)
}

/// Parse the checks array out of a stored doctor result.
pub fn checks_from_result(result: Option<&str>) -> Vec<Value> {
    result
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value.get("checks").cloned())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

async fn collect_checks(state: &AppState) -> Result<Vec<DoctorCheck>, sqlx::Error> {
    let mut checks = Vec::new();
    let pool = state.db().pool();

    // 1. Database integrity (read-only quick check; never repairs).
    let integrity: String = sqlx::query_scalar("PRAGMA quick_check(1)")
        .fetch_one(pool)
        .await?;
    checks.push(DoctorCheck {
        check_id: "database_integrity".to_owned(),
        label: "Database integrity".to_owned(),
        status: if integrity == "ok" {
            STATUS_PASS
        } else {
            STATUS_FAIL
        },
        detail: if integrity == "ok" {
            "SQLite quick_check reports a consistent database".to_owned()
        } else {
            format!("SQLite quick_check failed: {integrity}")
        },
    });

    // 2. Schema version matches the Server binary.
    let schema: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await?;
    checks.push(DoctorCheck {
        check_id: "schema_version".to_owned(),
        label: "Schema version".to_owned(),
        status: if schema == crate::database::SERVER_SCHEMA_VERSION {
            STATUS_PASS
        } else {
            STATUS_FAIL
        },
        detail: format!(
            "database schema {schema}, Server expects {}",
            crate::database::SERVER_SCHEMA_VERSION
        ),
    });

    // 3. Critical workers (delivery/alert/operations heartbeat).
    checks.push(DoctorCheck {
        check_id: "critical_workers".to_owned(),
        label: "Background workers".to_owned(),
        status: if state.critical_workers_healthy() {
            STATUS_PASS
        } else {
            STATUS_FAIL
        },
        detail: if state.critical_workers_healthy() {
            "Delivery, alert, and Operation workers report healthy heartbeats".to_owned()
        } else {
            "A background worker is unhealthy or stale; review readiness and logs".to_owned()
        },
    });

    // 4. Web assets (readiness component; missing assets are a warning).
    checks.push(DoctorCheck {
        check_id: "web_assets".to_owned(),
        label: "Web assets".to_owned(),
        status: if state.web_assets_ready() {
            STATUS_PASS
        } else if state.web_assets().is_some() {
            STATUS_WARNING
        } else {
            STATUS_NOT_CONFIGURED
        },
        detail: if state.web_assets_ready() {
            "index.html and hashed assets are present".to_owned()
        } else if state.web_assets().is_some() {
            "web root exists but index.html or the assets directory is missing".to_owned()
        } else {
            "no web root configured; the WebUI is not served".to_owned()
        },
    });

    // 5. Retention policies are seeded within safety bounds.
    let policy_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM retention_policies")
        .fetch_one(pool)
        .await?;
    let expected = crate::retention::POLICY_CATALOG.len() as i64;
    checks.push(DoctorCheck {
        check_id: "retention_policies".to_owned(),
        label: "Retention policies".to_owned(),
        status: if policy_count == expected {
            STATUS_PASS
        } else {
            STATUS_WARNING
        },
        detail: format!("{policy_count} of {expected} retention policy families are seeded"),
    });

    // 6. Backup storage: configured, present, and strictly permissioned.
    match state.backup_dir() {
        None => checks.push(DoctorCheck {
            check_id: "backup_storage".to_owned(),
            label: "Backup storage".to_owned(),
            status: STATUS_NOT_CONFIGURED,
            detail: "no backup directory configured; backup Operations will fail".to_owned(),
        }),
        Some(dir) => match std::fs::symlink_metadata(dir) {
            Ok(metadata) if metadata.is_dir() => {
                let safe = crate::file_security::validate_private_directory(dir).is_ok();
                checks.push(DoctorCheck {
                    check_id: "backup_storage".to_owned(),
                    label: "Backup storage".to_owned(),
                    status: if safe { STATUS_PASS } else { STATUS_FAIL },
                    detail: if safe {
                        "backup directory exists with strict ownership and permissions".to_owned()
                    } else {
                        "backup directory is not private and owned by the Server user".to_owned()
                    },
                });
            }
            Ok(_) => checks.push(DoctorCheck {
                check_id: "backup_storage".to_owned(),
                label: "Backup storage".to_owned(),
                status: STATUS_FAIL,
                detail: "configured backup path is not a directory".to_owned(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                checks.push(DoctorCheck {
                    check_id: "backup_storage".to_owned(),
                    label: "Backup storage".to_owned(),
                    status: STATUS_WARNING,
                    detail: "backup directory does not exist yet; the first backup creates it"
                        .to_owned(),
                })
            }
            Err(_) => checks.push(DoctorCheck {
                check_id: "backup_storage".to_owned(),
                label: "Backup storage".to_owned(),
                status: STATUS_FAIL,
                detail: "cannot access backup directory".to_owned(),
            }),
        },
    }

    // 7. Latest backup artifact integrity (skipped when nothing exists).
    let latest = crate::backup::latest_artifact(pool).await?;
    checks.push(match latest {
        Some((artifact_id, _filename, _bytes, verification)) => DoctorCheck {
            check_id: "latest_backup".to_owned(),
            label: "Latest backup".to_owned(),
            status: match verification.as_str() {
                "ok" => STATUS_PASS,
                "failed" => STATUS_FAIL,
                _ => STATUS_WARNING,
            },
            detail: match verification.as_str() {
                "ok" => format!("backup artifact {artifact_id} is verified"),
                "failed" => format!("backup artifact {artifact_id} failed verification"),
                _ => format!("backup artifact {artifact_id} has not been verified yet"),
            },
        },
        None => DoctorCheck {
            check_id: "latest_backup".to_owned(),
            label: "Latest backup".to_owned(),
            status: STATUS_SKIPPED,
            detail: "no backup artifact exists yet; nothing to verify".to_owned(),
        },
    });

    // 8. Notification channel configuration (never token contents).
    let telegram = state.channels().telegram.as_ref();
    checks.push(DoctorCheck {
        check_id: "notification_channels".to_owned(),
        label: "Notification channels".to_owned(),
        status: match telegram {
            Some(channel) if channel.enabled => STATUS_PASS,
            Some(_) => STATUS_NOT_CONFIGURED,
            None => STATUS_NOT_CONFIGURED,
        },
        detail: match telegram {
            Some(channel) if channel.enabled => {
                format!(
                    "Telegram delivery is enabled with bounded retry (max {} attempts)",
                    channel.max_attempts
                )
            }
            Some(_) => "a Telegram channel exists but is disabled".to_owned(),
            None => "no notification channels configured".to_owned(),
        },
    });

    // 9. Sensitive state files and optional Geo database are checked through
    // descriptor-based no-follow validation; failures are actionable but do
    // not include filesystem paths in the Admin response.
    let database_safe = crate::file_security::validate_file(state.db().path()).is_ok();
    checks.push(DoctorCheck {
        check_id: "database_storage".to_owned(),
        label: "Database storage".to_owned(),
        status: if database_safe {
            STATUS_PASS
        } else {
            STATUS_FAIL
        },
        detail: if database_safe {
            "Server database is a private regular file owned by the Server user".to_owned()
        } else {
            "Server database ownership or permissions are unsafe".to_owned()
        },
    });
    checks.push(match state.geo().path() {
        None => DoctorCheck {
            check_id: "geo_database".to_owned(),
            label: "Geo database".to_owned(),
            status: STATUS_NOT_CONFIGURED,
            detail: "no GeoLite database configured".to_owned(),
        },
        Some(path) if crate::file_security::validate_private_file(path).is_ok() => DoctorCheck {
            check_id: "geo_database".to_owned(),
            label: "Geo database".to_owned(),
            status: STATUS_PASS,
            detail: "configured GeoLite database is a private regular file".to_owned(),
        },
        Some(_) => DoctorCheck {
            check_id: "geo_database".to_owned(),
            label: "Geo database".to_owned(),
            status: STATUS_FAIL,
            detail: "configured GeoLite database ownership or permissions are unsafe".to_owned(),
        },
    });

    // 10. Sensitive file discipline is a unix property; elsewhere skipped.
    #[cfg(unix)]
    {
        checks.push(DoctorCheck {
            check_id: "platform_security".to_owned(),
            label: "Platform security".to_owned(),
            status: STATUS_PASS,
            detail: "unix file permission discipline applies to state, secrets, and backups"
                .to_owned(),
        });
    }
    #[cfg(not(unix))]
    checks.push(DoctorCheck {
        check_id: "platform_security".to_owned(),
        label: "Platform security".to_owned(),
        status: STATUS_SKIPPED,
        detail: "platform permission checks are not implemented on this OS".to_owned(),
    });

    Ok(checks)
}
