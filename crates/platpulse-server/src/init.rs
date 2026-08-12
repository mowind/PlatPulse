//! `platpulse-server init` (design §12.2, §18.1): create the state
//! directory, open and migrate the Server SQLite database, generate the
//! standalone pepper file, and validate file ownership/permissions and
//! unexpected symlinks before printing the next steps.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::ServerConfig;
use crate::database::{ServerDatabaseConfig, initialize};
use crate::secrets::{PepperError, create_pepper_file, load_pepper_file};

#[derive(Debug, Error)]
pub enum InitError {
    #[error("failed to prepare state directory {path}: {source}")]
    StateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "state directory {path} must be a real directory owned by the server user and not group/world-writable"
    )]
    UnsafeStateDirectory { path: PathBuf },
    #[error("database path {path} must not be a symlink or owned by another user")]
    UnsafeDatabasePath { path: PathBuf },
    #[error("failed to secure database file {path}: {source}")]
    SecureDatabase {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("server database initialization failed: {0}")]
    Database(#[from] crate::database::ServerDatabaseError),
    #[error(transparent)]
    Pepper(#[from] PepperError),
}

/// Result of `init`, with warnings that must not fail the run (a Server may
/// start without Web assets and report `web_assets_missing` instead).
#[derive(Debug, Default)]
pub struct InitReport {
    pub warnings: Vec<String>,
}

/// Run the local initialization flow. Safe to re-run: existing state is
/// validated, never overwritten.
pub async fn run_init(config: &ServerConfig) -> Result<InitReport, InitError> {
    restrict_umask();
    create_state_directory(&config.state_dir)?;

    // Open (creating if missing) and migrate the database; validate an
    // existing database file before SQLite touches it.
    validate_database_path(&config.db_path)?;
    let database = initialize(ServerDatabaseConfig::new(&config.db_path)).await?;
    database.close().await;
    secure_database_file(&config.db_path)?;

    // Pepper: generate once, validate on every later run.
    if config.pepper_file.exists() {
        load_pepper_file(&config.pepper_file)?;
    } else {
        create_pepper_file(&config.pepper_file)?;
    }

    let mut report = InitReport::default();
    if let Some(web_root) = &config.web_root {
        let index_ok = web_root.join("index.html").is_file();
        let assets_ok = web_root.join("assets").is_dir();
        if !index_ok || !assets_ok {
            report.warnings.push(format!(
                "web root {web_root:?} is incomplete (index.html and assets/ required); the Server will report web_assets_missing until the WebUI is installed"
            ));
        }
    }
    Ok(report)
}

/// Find the first ancestor of `path` (excluding `path` itself) that is a
/// symlink, walking from the root down. `create_dir_all` and SQLite follow
/// symlinked ancestors, so `init` must reject them before creating state
/// (design §18.1: no unexpected symlink substitution).
fn first_symlinked_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                if let Ok(metadata) = std::fs::symlink_metadata(parent) {
                    if metadata.file_type().is_symlink() {
                        return Some(parent.to_owned());
                    }
                }
                current = parent;
            }
            _ => return None,
        }
    }
}

/// Create the state directory and validate it: a real directory (not a
/// symlink), owned by the current user, not group/world-writable. A newly
/// created directory is restricted to the server user (0700).
fn create_state_directory(path: &Path) -> Result<(), InitError> {
    if let Some(symlink) = first_symlinked_ancestor(path) {
        return Err(InitError::UnsafeStateDirectory { path: symlink });
    }
    let created = match std::fs::symlink_metadata(path) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path).map_err(|source| InitError::StateDirectory {
                path: path.to_owned(),
                source,
            })?;
            true
        }
        Err(source) => {
            return Err(InitError::StateDirectory {
                path: path.to_owned(),
                source,
            });
        }
    };

    if created {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(
                |source| InitError::StateDirectory {
                    path: path.to_owned(),
                    source,
                },
            )?;
        }
    }

    let metadata = std::fs::symlink_metadata(path).map_err(|source| InitError::StateDirectory {
        path: path.to_owned(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(InitError::UnsafeStateDirectory {
            path: path.to_owned(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owned_by_server_user = metadata.uid() == nix::unistd::geteuid().as_raw();
        let not_group_or_world_writable = metadata.mode() & 0o022 == 0;
        if !owned_by_server_user || !not_group_or_world_writable {
            return Err(InitError::UnsafeStateDirectory {
                path: path.to_owned(),
            });
        }
    }
    Ok(())
}

/// Refuse to open a database path that is a symlink or owned by another
/// user (design §18.1's no-follow/open-then-stat policy, applied before
/// SQLite touches the file).
fn validate_database_path(path: &Path) -> Result<(), InitError> {
    if let Some(symlink) = first_symlinked_ancestor(path) {
        return Err(InitError::UnsafeDatabasePath { path: symlink });
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(InitError::UnsafeDatabasePath {
                    path: path.to_owned(),
                });
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.uid() != nix::unistd::geteuid().as_raw() {
                    return Err(InitError::UnsafeDatabasePath {
                        path: path.to_owned(),
                    });
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The database does not exist yet; ensure its parent directory
            // is available so SQLite can create the file, then verify the
            // parent is a real directory and not a smuggled-in symlink.
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|source| {
                        InitError::StateDirectory {
                            path: parent.to_owned(),
                            source,
                        }
                    })?;
                    let metadata = std::fs::symlink_metadata(parent).map_err(|source| {
                        InitError::StateDirectory {
                            path: parent.to_owned(),
                            source,
                        }
                    })?;
                    if metadata.file_type().is_symlink() {
                        return Err(InitError::UnsafeDatabasePath {
                            path: path.to_owned(),
                        });
                    }
                    // A freshly created database parent is restricted to
                    // the server user; an existing one only needs to be
                    // free of group/world write bits.
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if metadata.permissions().mode() & 0o022 != 0 {
                            std::fs::set_permissions(
                                parent,
                                std::fs::Permissions::from_mode(0o700),
                            )
                            .map_err(|source| {
                                InitError::StateDirectory {
                                    path: parent.to_owned(),
                                    source,
                                }
                            })?;
                        }
                    }
                }
            }
            Ok(())
        }
        Err(source) => Err(InitError::StateDirectory {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Restrict the database file to the server user (0600); WAL/SHM siblings
/// inherit the process umask, so `serve` also runs with umask 077.
pub(crate) fn secure_database_file(path: &Path) -> Result<(), InitError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
            |source| InitError::SecureDatabase {
                path: path.to_owned(),
                source,
            },
        )?;
    }
    Ok(())
}

/// Run with umask 077 so SQLite WAL/SHM siblings and any other files this
/// process creates inherit server-user-only permissions (design §18.1).
#[cfg(unix)]
pub(crate) fn restrict_umask() {
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
}

#[cfg(not(unix))]
pub(crate) fn restrict_umask() {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use crate::config::ServerConfig;
    use crate::secrets::load_pepper_file;

    use super::*;

    /// Write a config file that anchors state under `state_dir` (with the
    /// optional web root under `dir/web`) and resolve it like `init` does.
    fn init_config(dir: &std::path::Path, state_dir: &std::path::Path) -> (PathBuf, ServerConfig) {
        let config_path = dir.join("server.toml");
        fs::write(
            &config_path,
            format!(
                "state_dir = {:?}\nweb_root = {:?}\n",
                state_dir,
                dir.join("web")
            ),
        )
        .unwrap();
        let config = ServerConfig::resolve_init(&config_path).unwrap();
        (config_path, config)
    }

    #[tokio::test]
    async fn init_creates_state_database_and_pepper() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("web")).unwrap();
        fs::write(dir.path().join("web/index.html"), "<!doctype html>").unwrap();
        fs::create_dir(dir.path().join("web/assets")).unwrap();

        let state = dir.path().join("state");
        let (_, config) = init_config(dir.path(), &state);
        let report = run_init(&config).await.unwrap();
        assert!(report.warnings.is_empty());

        assert!(state.is_dir());
        let metadata = fs::symlink_metadata(&state).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        assert!(config.db_path.is_file());
        let db_metadata = fs::metadata(&config.db_path).unwrap();
        assert_eq!(db_metadata.permissions().mode() & 0o777, 0o600);

        // Pepper exists, is 0600, and loads.
        let pepper = load_pepper_file(&config.pepper_file).unwrap();
        let pepper_metadata = fs::metadata(&config.pepper_file).unwrap();
        assert_eq!(pepper_metadata.permissions().mode() & 0o777, 0o600);

        // Schema is migrated.
        let database = initialize(ServerDatabaseConfig::new(&config.db_path))
            .await
            .unwrap();
        assert_eq!(
            database.schema_version().await.unwrap(),
            crate::database::SERVER_SCHEMA_VERSION
        );
        database.close().await;

        // Idempotent re-run keeps the same pepper.
        let rerun = run_init(&config).await.unwrap();
        assert!(rerun.warnings.is_empty());
        assert_eq!(load_pepper_file(&config.pepper_file).unwrap(), pepper);
    }

    #[tokio::test]
    async fn init_warns_but_does_not_fail_when_web_assets_are_missing() {
        let dir = tempdir().unwrap();
        let (_, config) = init_config(dir.path(), &dir.path().join("state"));
        let report = run_init(&config).await.unwrap();
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("web_assets_missing"));
    }

    #[tokio::test]
    async fn init_rejects_symlinked_state_dir() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real-state");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("state-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let (_, config) = init_config(dir.path(), &link);
        let error = run_init(&config).await.unwrap_err();
        assert!(matches!(error, InitError::UnsafeStateDirectory { .. }));
    }

    #[tokio::test]
    async fn init_rejects_symlinked_database() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        let target = dir.path().join("real.db");
        fs::write(&target, "not sqlite").unwrap();
        let link = state.join("platpulse.db");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let config_path = dir.path().join("server.toml");
        fs::write(
            &config_path,
            format!("state_dir = {:?}\ndb_path = {:?}\n", state, link),
        )
        .unwrap();
        let config = ServerConfig::resolve_init(&config_path).unwrap();
        let error = run_init(&config).await.unwrap_err();
        assert!(matches!(error, InitError::UnsafeDatabasePath { .. }));
    }

    #[tokio::test]
    async fn init_rejects_symlinked_database_parent() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real-state");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("state-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let config_path = dir.path().join("server.toml");
        fs::write(
            &config_path,
            format!(
                "state_dir = {:?}\ndb_path = {:?}\n",
                dir.path().join("state"),
                link.join("platpulse.db")
            ),
        )
        .unwrap();
        let config = ServerConfig::resolve_init(&config_path).unwrap();
        let error = run_init(&config).await.unwrap_err();
        assert!(matches!(error, InitError::UnsafeDatabasePath { .. }));
    }
}
