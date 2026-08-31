//! Bounded measurement of an explicitly configured Node data directory.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use platpulse_core::component::{BoundedError, ComponentObservation, ComponentStatus};
use platpulse_core::time::Rfc3339;

pub const DATA_DIRECTORY_SAMPLE_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq)]
pub struct DataDirectoryObservations {
    pub size_bytes: ComponentObservation<u64>,
    pub capacity_bytes: ComponentObservation<u64>,
}

pub fn disabled_observations() -> DataDirectoryObservations {
    DataDirectoryObservations {
        size_bytes: disabled(),
        capacity_bytes: disabled(),
    }
}

pub fn starting_observations() -> DataDirectoryObservations {
    DataDirectoryObservations {
        size_bytes: starting(),
        capacity_bytes: starting(),
    }
}

pub fn disabled() -> ComponentObservation<u64> {
    ComponentObservation {
        status: ComponentStatus::Disabled,
        attempted_at: None,
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: None,
    }
}

pub fn starting() -> ComponentObservation<u64> {
    ComponentObservation {
        status: ComponentStatus::Starting,
        attempted_at: None,
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: None,
    }
}

/// Measure one configured PlatON data directory and the total capacity of its
/// containing filesystem. Regular-file lengths are summed recursively;
/// symlinks are never followed. Failures expose bounded diagnostics without
/// leaking the configured path.
pub fn collect_observations(path: &Path, attempted_at: Rfc3339) -> DataDirectoryObservations {
    let never_cancelled = AtomicBool::new(false);
    collect_observations_cancellable(path, attempted_at, &never_cancelled)
}

pub fn collect_observations_cancellable(
    path: &Path,
    attempted_at: Rfc3339,
    cancelled: &AtomicBool,
) -> DataDirectoryObservations {
    DataDirectoryObservations {
        size_bytes: collect_cancellable(path, attempted_at, cancelled),
        capacity_bytes: collect_capacity(path, attempted_at, cancelled),
    }
}

pub fn collect(path: &Path, attempted_at: Rfc3339) -> ComponentObservation<u64> {
    let never_cancelled = AtomicBool::new(false);
    collect_cancellable(path, attempted_at, &never_cancelled)
}

fn collect_cancellable(
    path: &Path,
    attempted_at: Rfc3339,
    cancelled: &AtomicBool,
) -> ComponentObservation<u64> {
    match directory_size(path, cancelled) {
        Ok(bytes) if bytes <= i64::MAX as u64 => ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(attempted_at),
            latest_observed_at: Some(attempted_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(bytes),
            error: None,
        },
        Ok(_) | Err(_) => failed(attempted_at),
    }
}

fn collect_capacity(
    path: &Path,
    attempted_at: Rfc3339,
    cancelled: &AtomicBool,
) -> ComponentObservation<u64> {
    if cancelled.load(Ordering::Acquire) {
        return capacity_failed(attempted_at);
    }
    match file_system_capacity(path) {
        Ok(bytes) if bytes <= i64::MAX as u64 => ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(attempted_at),
            latest_observed_at: Some(attempted_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(bytes),
            error: None,
        },
        Ok(_) | Err(_) => capacity_failed(attempted_at),
    }
}

pub fn failed_observations(attempted_at: Rfc3339) -> DataDirectoryObservations {
    DataDirectoryObservations {
        size_bytes: failed(attempted_at),
        capacity_bytes: capacity_failed(attempted_at),
    }
}

pub fn failed(attempted_at: Rfc3339) -> ComponentObservation<u64> {
    ComponentObservation {
        status: ComponentStatus::Error,
        attempted_at: Some(attempted_at),
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: Some(BoundedError {
            code: "data_directory_scan_failed".to_owned(),
            message: "PlatON data directory could not be measured".to_owned(),
        }),
    }
}

fn capacity_failed(attempted_at: Rfc3339) -> ComponentObservation<u64> {
    ComponentObservation {
        status: ComponentStatus::Error,
        attempted_at: Some(attempted_at),
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: Some(BoundedError {
            code: "data_directory_capacity_failed".to_owned(),
            message: "PlatON data directory filesystem capacity could not be measured".to_owned(),
        }),
    }
}

fn validate_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "data directory is not a real directory",
        ));
    }
    Ok(())
}

fn directory_size(path: &Path, cancelled: &AtomicBool) -> io::Result<u64> {
    validate_directory(path)?;
    sum_directory(path, cancelled)
}

#[cfg(unix)]
fn file_system_capacity(path: &Path) -> io::Result<u64> {
    validate_directory(path)?;
    let statistics = nix::sys::statvfs::statvfs(path).map_err(io::Error::other)?;
    statistics
        .blocks()
        .checked_mul(statistics.fragment_size())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "filesystem capacity overflow"))
}

#[cfg(not(unix))]
fn file_system_capacity(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "filesystem capacity is unsupported on this platform",
    ))
}

fn sum_directory(path: &Path, cancelled: &AtomicBool) -> io::Result<u64> {
    if cancelled.load(Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
        }
        let entry = entry?;
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            total = total
                .checked_add(sum_directory(&entry_path, cancelled)?)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "data directory size overflow")
                })?;
        } else if file_type.is_file() {
            total = total.checked_add(metadata.len()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "data directory size overflow")
            })?;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn measures_regular_files_recursively_without_following_symlinks() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("a"), [0_u8; 3]).unwrap();
        fs::write(dir.path().join("nested/b"), [0_u8; 5]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("nested"), dir.path().join("link")).unwrap();

        let value = collect(dir.path(), "2026-08-12T10:00:00Z".parse().unwrap());
        assert_eq!(value.status, ComponentStatus::Ok);
        assert_eq!(value.latest, Some(8));
    }

    #[test]
    fn rejects_a_symlinked_root_without_leaking_its_path() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        #[cfg(unix)]
        {
            let link = dir.path().join("node-data-secret");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let value = collect(&link, "2026-08-12T10:00:00Z".parse().unwrap());
            assert_eq!(value.status, ComponentStatus::Error);
            assert!(value.latest.is_none());
            assert!(!value.error.unwrap().message.contains("node-data-secret"));
        }
    }
}
