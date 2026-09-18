//! Server-owned online backup schedule (design §20.1).
//!
//! The Server snapshots its own database on the single owning connection, so
//! automatic backups need no Owner credential, no machine token, and no
//! second process that would open a live SQLite file. Every automatic
//! artifact is verified (checksum, read-only integrity, schema) before the
//! schedule records success, and creation keeps every previous artifact.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::BackupScheduleConfig;
use crate::http::AppState;

/// Persisted schedule state keys in `server_settings`.
const SETTING_LAST_SUCCESS: &str = "backup_schedule_last_success_at";
const SETTING_LAST_ATTEMPT: &str = "backup_schedule_last_attempt_at";
const SETTING_LAST_ARTIFACT: &str = "backup_schedule_last_artifact";
const SETTING_LAST_ERROR: &str = "backup_schedule_last_error";

/// How often the cheap due-time check runs. A due check touches no backup
/// file, so a short cadence never blocks the single SQLite connection.
const TICK: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
enum ScheduleError {
    #[error("schedule database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("scheduled backup error: {0}")]
    Backup(#[from] crate::backup::BackupError),
}

/// Run the schedule until shutdown. A configured schedule is Server-owned and
/// in-process: it never opens a second connection to the live database.
pub async fn run(state: AppState, config: BackupScheduleConfig) {
    run_with_device(state, config, device_of).await;
}

async fn run_with_device<F>(state: AppState, config: BackupScheduleConfig, device: F)
where
    F: Fn(&Path) -> std::io::Result<u64>,
{
    let started = std::time::Instant::now();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if state.is_shutting_down() {
            return;
        }
        tokio::select! {
            _ = state.shutdown_signal() => return,
            _ = tick.tick() => {}
        }
        if state.is_shutting_down() {
            return;
        }
        match attempt_if_due(&state, &config, started, &device).await {
            Ok(()) => {}
            Err(error) => eprintln!(
                "backup schedule deferred: {}",
                crate::redaction::redact_sensitive(&error.to_string())
            ),
        }
    }
}

/// Run one due check and, when due, one guarded create-then-verify pass.
async fn attempt_if_due<F>(
    state: &AppState,
    config: &BackupScheduleConfig,
    started: std::time::Instant,
    device: &F,
) -> Result<(), ScheduleError>
where
    F: Fn(&Path) -> std::io::Result<u64>,
{
    let now = crate::auth::now_utc();
    let last_success = read_setting(state, SETTING_LAST_SUCCESS).await?;
    let last_attempt = read_setting(state, SETTING_LAST_ATTEMPT).await?;
    if !is_due(
        now,
        last_success.as_deref(),
        last_attempt.as_deref(),
        config,
        started,
    ) {
        return Ok(());
    }

    // The layout guard runs before any snapshot attempt so an absent or
    // unmounted backup disk fails closed instead of writing beside the
    // database or onto the wrong filesystem.
    if let Err(reason) = check_layout(
        state.db().path(),
        state.backup_dir(),
        &config.required_mount,
        device,
    ) {
        let now_text = crate::auth::format_rfc3339(now);
        record_attempt(state, &now_text, Some(&reason)).await?;
        eprintln!(
            "backup schedule guard rejected: {}",
            crate::redaction::redact_sensitive(&reason)
        );
        return Ok(());
    }

    let artifact = crate::backup::create_scheduled(state).await?;
    let verified = crate::backup::verify_scheduled_artifact(state, &artifact.artifact_id).await?;
    let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
    if verified {
        record_success(state, &now_text, &artifact.filename).await?;
        println!("backup schedule created and verified {}", artifact.filename);
    } else {
        record_attempt(state, &now_text, Some("automatic verification failed")).await?;
        eprintln!("backup schedule verification failed for a fresh artifact");
    }
    Ok(())
}

/// Decide whether an attempt is due from durable timestamps. Pure so tests
/// pin both the success interval and the failure backoff without a clock.
fn is_due(
    now: time::OffsetDateTime,
    last_success: Option<&str>,
    last_attempt: Option<&str>,
    config: &BackupScheduleConfig,
    started: std::time::Instant,
) -> bool {
    if let Some(success) = last_success {
        let Some(success_at) = crate::auth::parse_rfc3339(success) else {
            // An unreadable timestamp must not silently disable backups.
            return true;
        };
        return now >= success_at + config.interval;
    }
    if started.elapsed() < BackupScheduleConfig::INITIAL_DELAY {
        return false;
    }
    match last_attempt.and_then(crate::auth::parse_rfc3339) {
        Some(attempt_at) => now >= attempt_at + BackupScheduleConfig::RETRY_AFTER,
        None => true,
    }
}

/// Canonical layout facts the guard decides over.
struct LayoutFacts {
    backup_dir: PathBuf,
    required_mount: PathBuf,
    backup_device: u64,
    mount_device: u64,
    mount_parent_device: u64,
    db_device: u64,
}

/// Pure guard decision over canonical facts.
fn decide_layout(facts: &LayoutFacts) -> Result<(), String> {
    if !facts.backup_dir.starts_with(&facts.required_mount) {
        return Err("backup directory is outside required_mount".to_owned());
    }
    if facts.mount_device == facts.mount_parent_device {
        return Err("required_mount is not a separate mounted filesystem".to_owned());
    }
    if facts.backup_device != facts.mount_device {
        return Err("backup directory is not on required_mount".to_owned());
    }
    if facts.backup_device == facts.db_device {
        return Err("backup directory is on the database filesystem".to_owned());
    }
    Ok(())
}

/// Canonicalize and read the filesystem identity of each relevant path, then
/// apply [`decide_layout`].
fn check_layout<F>(
    db_path: &Path,
    backup_dir: Option<&PathBuf>,
    required_mount: &Path,
    device: &F,
) -> Result<(), String>
where
    F: Fn(&Path) -> std::io::Result<u64>,
{
    let Some(backup_dir) = backup_dir else {
        return Err("backup_dir is not configured".to_owned());
    };
    if !required_mount.is_absolute() {
        return Err("required_mount is not absolute".to_owned());
    }
    let canonical_mount = std::fs::canonicalize(required_mount)
        .map_err(|_| "required_mount is not accessible".to_owned())?;
    let mount_device =
        device(&canonical_mount).map_err(|_| "required_mount is not accessible".to_owned())?;
    let mount_parent_device = canonical_mount
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(|parent| device(parent).ok())
        .unwrap_or(mount_device);
    crate::file_security::ensure_private_directory(backup_dir)
        .map_err(|_| "backup directory is unsafe or could not be created".to_owned())?;
    let canonical_backup = std::fs::canonicalize(backup_dir)
        .map_err(|_| "backup directory is not accessible".to_owned())?;
    let backup_device =
        device(&canonical_backup).map_err(|_| "backup directory is not accessible".to_owned())?;
    let db_device = device(db_path)
        .or_else(|_| {
            db_path
                .parent()
                .ok_or_else(|| std::io::Error::other("database path has no parent"))
                .and_then(device)
        })
        .map_err(|_| "database path is not accessible".to_owned())?;
    decide_layout(&LayoutFacts {
        backup_dir: canonical_backup,
        required_mount: canonical_mount,
        backup_device,
        mount_device,
        mount_parent_device,
        db_device,
    })
}

/// Real filesystem identity. `st_dev` is stable for mounted filesystems, so a
/// renamed or unmounted directory cannot impersonate the expected disk.
fn device_of(path: &Path) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(path)?.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::other(
            "device identity is only available on unix",
        ))
    }
}

async fn read_setting(state: &AppState, key: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT setting_value FROM server_settings WHERE setting_key = ?")
        .bind(key)
        .fetch_optional(state.db().pool())
        .await
}

async fn write_setting(state: &AppState, key: &str, value: &str) -> Result<(), sqlx::Error> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    sqlx::query(
        "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES (?, ?, ?) ON CONFLICT(setting_key) DO UPDATE SET setting_value = excluded.setting_value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(now)
    .execute(state.db().pool())
    .await
    .map(|_| ())
}

async fn record_success(state: &AppState, now: &str, filename: &str) -> Result<(), sqlx::Error> {
    write_setting(state, SETTING_LAST_SUCCESS, now).await?;
    write_setting(state, SETTING_LAST_ATTEMPT, now).await?;
    write_setting(state, SETTING_LAST_ARTIFACT, filename).await?;
    write_setting(state, SETTING_LAST_ERROR, "").await
}

async fn record_attempt(
    state: &AppState,
    now: &str,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    write_setting(state, SETTING_LAST_ATTEMPT, now).await?;
    if let Some(error) = error {
        write_setting(state, SETTING_LAST_ERROR, error).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthConfig;
    use crate::database::{ServerDatabaseConfig, initialize};
    use crate::secrets::{create_pepper_file, load_pepper_file};

    fn facts(backup: &str, mount: &str, bdev: u64, mdev: u64, pdev: u64, ddev: u64) -> LayoutFacts {
        LayoutFacts {
            backup_dir: PathBuf::from(backup),
            required_mount: PathBuf::from(mount),
            backup_device: bdev,
            mount_device: mdev,
            mount_parent_device: pdev,
            db_device: ddev,
        }
    }

    #[test]
    fn decide_layout_accepts_a_separate_mounted_disk() {
        assert!(decide_layout(&facts("/data/backups", "/data", 2, 2, 1, 3)).is_ok());
    }

    #[test]
    fn decide_layout_rejects_each_unsafe_layout() {
        // Backup outside the required mount.
        assert!(decide_layout(&facts("/srv/backups", "/data", 2, 2, 1, 3)).is_err());
        // required_mount is a plain directory, not a real mount point.
        assert!(decide_layout(&facts("/data/backups", "/data", 2, 2, 2, 3)).is_err());
        // Backup directory is not on the required mount device.
        assert!(decide_layout(&facts("/data/backups", "/data", 9, 2, 1, 3)).is_err());
        // Backup shares the live database filesystem.
        assert!(decide_layout(&facts("/data/backups", "/data", 3, 3, 1, 3)).is_err());
    }

    fn schedule_config(mount: &Path) -> BackupScheduleConfig {
        BackupScheduleConfig {
            interval: Duration::from_secs(86_400),
            required_mount: mount.to_path_buf(),
        }
    }

    #[test]
    fn is_due_waits_out_initial_delay_then_backs_off_failures() {
        let config = schedule_config(Path::new("/data"));
        let now = crate::auth::now_utc();
        assert!(!is_due(now, None, None, &config, std::time::Instant::now()));
        let settled = std::time::Instant::now() - Duration::from_secs(300);
        assert!(is_due(now, None, None, &config, settled));
        let recent = crate::auth::format_rfc3339(now - Duration::from_secs(60));
        assert!(!is_due(now, None, Some(&recent), &config, settled));
        let stale = crate::auth::format_rfc3339(now - Duration::from_secs(7_200));
        assert!(is_due(now, None, Some(&stale), &config, settled));
    }

    #[test]
    fn is_due_tracks_the_success_interval() {
        let config = schedule_config(Path::new("/data"));
        let now = crate::auth::now_utc();
        let recent = crate::auth::format_rfc3339(now - Duration::from_secs(3_600));
        assert!(!is_due(
            now,
            Some(&recent),
            None,
            &config,
            std::time::Instant::now()
        ));
        let old = crate::auth::format_rfc3339(now - Duration::from_secs(90_000));
        assert!(is_due(
            now,
            Some(&old),
            None,
            &config,
            std::time::Instant::now()
        ));
    }

    async fn state_with_backup() -> (tempfile::TempDir, tempfile::TempDir, AppState) {
        let db_dir = tempfile::TempDir::new().unwrap();
        let mount = tempfile::TempDir::new().unwrap();
        let backup = mount.path().join("backups");
        let database = initialize(ServerDatabaseConfig::new(db_dir.path().join("server.db")))
            .await
            .unwrap();
        let pepper = db_dir.path().join("pepper");
        create_pepper_file(&pepper).unwrap();
        let auth = AuthConfig::development(
            load_pepper_file(&pepper).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth).with_backup_dir(Some(backup));
        (db_dir, mount, state)
    }

    async fn artifact_count(state: &AppState) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM backup_artifacts")
            .fetch_one(state.db().pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn attempt_creates_and_verifies_one_backup_on_a_separate_disk() {
        let (_db_dir, mount, state) = state_with_backup().await;
        let mount_canon = std::fs::canonicalize(mount.path()).unwrap();
        let mount_parent = mount_canon.parent().unwrap().to_path_buf();
        let db_path = state.db().path().to_path_buf();
        let device = move |path: &Path| -> std::io::Result<u64> {
            if path == db_path.as_path() {
                return Ok(3);
            }
            if path == mount_parent.as_path() {
                return Ok(1);
            }
            if path == mount_canon.as_path() || path.starts_with(&mount_canon) {
                return Ok(2);
            }
            Ok(4)
        };
        let config = schedule_config(mount.path());
        let started = std::time::Instant::now() - Duration::from_secs(300);
        attempt_if_due(&state, &config, started, &device)
            .await
            .unwrap();

        let verification: String = sqlx::query_scalar(
            "SELECT verification FROM backup_artifacts ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(verification, "ok");
        let last_artifact: Option<String> =
            sqlx::query_scalar("SELECT setting_value FROM server_settings WHERE setting_key = ?")
                .bind(SETTING_LAST_ARTIFACT)
                .fetch_optional(state.db().pool())
                .await
                .unwrap();
        assert!(last_artifact.is_some());
    }

    #[tokio::test]
    async fn attempt_is_skipped_until_due() {
        let (_db_dir, mount, state) = state_with_backup().await;
        let device = |_path: &Path| Ok(2u64);
        let config = schedule_config(mount.path());
        attempt_if_due(&state, &config, std::time::Instant::now(), &device)
            .await
            .unwrap();
        assert_eq!(artifact_count(&state).await, 0);
    }

    #[tokio::test]
    async fn attempt_fails_closed_when_the_expected_mount_is_absent() {
        let (_db_dir, mount, state) = state_with_backup().await;
        let mount_canon = std::fs::canonicalize(mount.path()).unwrap();
        let mount_parent = mount_canon.parent().unwrap().to_path_buf();
        let device = move |path: &Path| -> std::io::Result<u64> {
            if path == mount_parent.as_path()
                || path == mount_canon.as_path()
                || path.starts_with(&mount_canon)
            {
                return Ok(7);
            }
            Ok(8)
        };
        let config = schedule_config(mount.path());
        let started = std::time::Instant::now() - Duration::from_secs(300);
        attempt_if_due(&state, &config, started, &device)
            .await
            .unwrap();
        assert_eq!(artifact_count(&state).await, 0);
        let error: Option<String> =
            sqlx::query_scalar("SELECT setting_value FROM server_settings WHERE setting_key = ?")
                .bind(SETTING_LAST_ERROR)
                .fetch_optional(state.db().pool())
                .await
                .unwrap();
        assert_eq!(
            error.as_deref(),
            Some("required_mount is not a separate mounted filesystem")
        );
    }

    #[tokio::test]
    async fn real_layout_guard_rejects_a_plain_directory() {
        let (_db_dir, mount, state) = state_with_backup().await;
        let backup = state.backup_dir().cloned().unwrap();
        let error =
            check_layout(state.db().path(), Some(&backup), mount.path(), &device_of).unwrap_err();
        assert_eq!(error, "required_mount is not a separate mounted filesystem");
    }
}
