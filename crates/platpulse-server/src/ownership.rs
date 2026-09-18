//! Server database ownership guard (issue #160, following #137).
//!
//! A serving Server holds an exclusive, non-blocking `flock` on
//! `<db_path>.lock` for its process lifetime. SQLite's own locking is not a
//! substitute for rejecting a second process *before* SQLite opens: an
//! external connection can become the first write-ahead-log shared-memory
//! attacher, truncate `-shm`, and crash the owner (issue #137). Every
//! offline command that opens the Server database therefore takes this same
//! guard before SQLite opens and holds it until the database is closed.
//!
//! Development mode is the deliberate local-tooling exception (it mirrors
//! [`crate::database::ServerDatabaseConfig::for_deployment`]): local tools
//! and the browser e2e fixture refreshers attach to a running dev Server, so
//! the CLI does not enforce the guard there. `serve` always holds it.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};
use thiserror::Error;

/// Why a command could not take ownership of the Server database.
#[derive(Debug, Error)]
pub enum OwnershipError {
    /// A running Server (or another exclusive command) already holds the
    /// guard. The caller must refuse *before* SQLite opens.
    #[error(
        "a running PlatPulse Server owns the Server database; stop the Server before running this command (an exclusive stopped-Server condition is required)"
    )]
    ServerRunning,
    #[error("database ownership guard could not be created: {0}")]
    Io(#[from] std::io::Error),
    #[error("database ownership guard path is unsafe: {0}")]
    Unsafe(String),
}

/// Lock file living next to the database (`<db_path>.lock`).
fn lock_path_for(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// An exclusive, non-blocking lock on the database. Holds the file
/// description for the caller's lifetime.
#[derive(Debug)]
pub struct OwnershipGuard {
    _lock: Flock<std::fs::File>,
}

/// Acquire the exclusive database ownership guard; `Err(ServerRunning)`
/// when a Server (or another exclusive command) already holds it.
pub fn acquire(db_path: &Path) -> Result<OwnershipGuard, OwnershipError> {
    let parent = db_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::file_security::validate_private_directory(parent).map_err(OwnershipError::Unsafe)?;
    let lock_path = lock_path_for(db_path);
    crate::file_security::validate_no_symlinked_ancestors(&lock_path)
        .map_err(OwnershipError::Unsafe)?;
    if std::fs::symlink_metadata(&lock_path).is_ok() {
        crate::file_security::validate_file(&lock_path).map_err(OwnershipError::Unsafe)?;
    }
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .mode(0o600)
            .open(&lock_path)?
    };
    #[cfg(not(unix))]
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    crate::file_security::validate_file(&lock_path).map_err(OwnershipError::Unsafe)?;
    let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock)
        .map_err(|_| OwnershipError::ServerRunning)?;
    Ok(OwnershipGuard { _lock: lock })
}

/// Acquire the ownership guard for a resolved deployment before opening
/// SQLite. Production deployments return `Some(guard)`; development mode is
/// the deliberate exception and returns `None` without creating a lock.
pub fn acquire_for_deployment(
    db_path: &Path,
    development: bool,
) -> Result<Option<OwnershipGuard>, OwnershipError> {
    if development {
        return Ok(None);
    }
    acquire(db_path).map(Some)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn a_second_owner_is_refused_until_the_guard_is_dropped() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("server.db");
        let guard = acquire(&db_path).unwrap();
        assert!(matches!(
            acquire(&db_path),
            Err(OwnershipError::ServerRunning)
        ));
        drop(guard);
        assert!(acquire(&db_path).is_ok());
    }

    #[test]
    fn development_mode_does_not_enforce_the_guard() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("server.db");
        let production = acquire_for_deployment(&db_path, false).unwrap();
        assert!(production.is_some());
        // Development mode ignores the held guard (local tooling attaches to
        // a running dev Server).
        assert!(acquire_for_deployment(&db_path, true).unwrap().is_none());
        assert!(matches!(
            acquire_for_deployment(&db_path, false),
            Err(OwnershipError::ServerRunning)
        ));
        drop(production);
        assert!(acquire_for_deployment(&db_path, false).is_ok());
    }
}
