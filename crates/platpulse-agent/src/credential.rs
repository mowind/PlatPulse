//! Agent Credential file (design §8.2, §12.6).
//!
//! The credential is separated from the ordinary TOML configuration and
//! stored in its own file restricted to the Agent OS user. `enroll` creates
//! it exclusively with mode 0600 and a no-follow open; every later load
//! applies the same open-then-stat policy as the Server pepper file:
//! symlinks, non-regular files, files owned by another user, and
//! group/world-readable files are rejected. The plaintext credential never
//! appears in argv, URLs, logs, or errors.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Agent Credential token prefix (`pp_agent_…`, design §12.5).
pub const AGENT_CREDENTIAL_PREFIX: &str = "pp_agent_";

/// Exact length of a valid credential token:
/// `pp_agent_` + UUID (36) + `_` + 64 lowercase hex (256-bit secret).
pub const AGENT_CREDENTIAL_TOKEN_LEN: usize = AGENT_CREDENTIAL_PREFIX.len() + 36 + 1 + 64;

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("credential file already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("failed to create credential file {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open credential file {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "credential file {path} must be a regular file owned by the agent user with no group or world access"
    )]
    Unsafe { path: PathBuf },
    #[error("credential file {path} does not contain a valid Agent Credential")]
    Malformed { path: PathBuf },
    #[error("failed to read credential file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Write the credential token to a new file (never overwrites) with mode
/// 0600 and a no-follow create, then fsync it so a crash cannot leave a
/// missing or truncated credential behind.
pub fn write_credential_file(path: &Path, token: &str) -> Result<(), CredentialError> {
    validate_token_shape(token).map_err(|_| CredentialError::Malformed {
        path: path.to_owned(),
    })?;

    #[cfg(unix)]
    let file = open_new_nofollow(path)?;
    #[cfg(not(unix))]
    let file = {
        use std::fs::OpenOptions;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| {
                if source.kind() == std::io::ErrorKind::AlreadyExists {
                    CredentialError::AlreadyExists(path.to_owned())
                } else {
                    CredentialError::Create {
                        path: path.to_owned(),
                        source,
                    }
                }
            })?
    };

    let mut file = file;
    file.write_all(token.as_bytes())
        .map_err(|source| CredentialError::Create {
            path: path.to_owned(),
            source,
        })?;
    file.write_all(b"\n")
        .map_err(|source| CredentialError::Create {
            path: path.to_owned(),
            source,
        })?;
    file.sync_all().map_err(|source| CredentialError::Create {
        path: path.to_owned(),
        source,
    })?;
    Ok(())
}

/// Load and validate the credential file with the no-follow open-then-stat
/// policy. Returns the plaintext credential; callers must treat it as a
/// secret and never log it.
pub fn load_credential_file(path: &Path) -> Result<String, CredentialError> {
    #[cfg(unix)]
    let file = open_nofollow_readonly(path)?;
    #[cfg(not(unix))]
    let file = File::open(path).map_err(|source| CredentialError::Open {
        path: path.to_owned(),
        source,
    })?;

    #[cfg(unix)]
    assert_safe_regular_file(&file, path)?;

    let mut content = String::new();
    file.take(1024)
        .read_to_string(&mut content)
        .map_err(|source| CredentialError::Read {
            path: path.to_owned(),
            source,
        })?;
    let token = content.trim_end_matches(['\n', '\r']).to_owned();
    validate_token_shape(&token).map_err(|_| CredentialError::Malformed {
        path: path.to_owned(),
    })?;
    Ok(token)
}

/// Structural check for a `pp_agent_<uuid>_<64 lowercase hex>` token. The
/// uuid part is only checked for non-empty shape; the Server is the
/// authority on the identity, and digest comparison happens there.
fn validate_token_shape(token: &str) -> Result<(), ()> {
    let Some(rest) = token.strip_prefix(AGENT_CREDENTIAL_PREFIX) else {
        return Err(());
    };
    if token.len() != AGENT_CREDENTIAL_TOKEN_LEN {
        return Err(());
    }
    let (token_id, secret) = rest.split_once('_').ok_or(())?;
    if token_id.len() != 36 || secret.len() != 64 {
        return Err(());
    }
    if !secret
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(());
    }
    Ok(())
}

#[cfg(unix)]
fn open_new_nofollow(path: &Path) -> Result<File, CredentialError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                CredentialError::AlreadyExists(path.to_owned())
            } else {
                CredentialError::Create {
                    path: path.to_owned(),
                    source,
                }
            }
        })?;
    assert_safe_regular_file(&file, path)?;
    Ok(file)
}

#[cfg(unix)]
fn open_nofollow_readonly(path: &Path) -> Result<File, CredentialError> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)
        .map_err(|source| CredentialError::Open {
            path: path.to_owned(),
            source,
        })
}

/// Open-then-stat: the opened descriptor must be a regular file owned by
/// the current user with no group/world access bits (design §18.1 policy
/// applied to the Agent credential).
#[cfg(unix)]
fn assert_safe_regular_file(file: &File, path: &Path) -> Result<(), CredentialError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|source| CredentialError::Read {
        path: path.to_owned(),
        source,
    })?;
    let mode = metadata.mode();
    let is_regular = metadata.file_type().is_file();
    let owned_by_agent_user = metadata.uid() == nix::unistd::geteuid().as_raw();
    let no_group_or_world_access = mode & 0o077 == 0;
    if !is_regular || !owned_by_agent_user || !no_group_or_world_access {
        return Err(CredentialError::Unsafe {
            path: path.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    /// A structurally valid credential token for tests.
    fn sample_token() -> String {
        format!(
            "{AGENT_CREDENTIAL_PREFIX}{}_{}",
            "0".repeat(36),
            "a".repeat(64)
        )
    }

    #[test]
    fn write_and_load_roundtrip_with_restrictive_permissions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credential");
        let token = sample_token();
        write_credential_file(&path, &token).unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            0o600,
            "credential file must be 0600"
        );
        assert_eq!(load_credential_file(&path).unwrap(), token);
    }

    #[test]
    fn write_refuses_to_overwrite_an_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credential");
        write_credential_file(&path, &sample_token()).unwrap();
        let error = write_credential_file(&path, &sample_token()).unwrap_err();
        assert!(matches!(error, CredentialError::AlreadyExists(_)));
    }

    #[test]
    fn write_rejects_malformed_tokens() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credential");
        let error = write_credential_file(&path, "not-a-token").unwrap_err();
        assert!(matches!(error, CredentialError::Malformed { .. }));
        assert!(
            !path.exists(),
            "a malformed token must not leave a credential file behind"
        );
    }

    #[test]
    fn load_rejects_group_readable_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credential");
        write_credential_file(&path, &sample_token()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let error = load_credential_file(&path).unwrap_err();
        assert!(matches!(error, CredentialError::Unsafe { .. }));
    }

    #[test]
    fn load_rejects_symlinked_credential() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("real-credential");
        write_credential_file(&target, &sample_token()).unwrap();
        let link = dir.path().join("credential");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error = load_credential_file(&link).unwrap_err();
        assert!(
            matches!(
                error,
                CredentialError::Open { .. } | CredentialError::Unsafe { .. }
            ),
            "symlinked credential must not be followed: {error}"
        );
    }

    #[test]
    fn load_rejects_malformed_and_truncated_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credential");
        write_credential_file(&path, &sample_token()).unwrap();
        std::fs::write(&path, "garbage").unwrap();
        assert!(matches!(
            load_credential_file(&path).unwrap_err(),
            CredentialError::Malformed { .. }
        ));
    }

    #[test]
    fn token_shape_validation() {
        assert!(validate_token_shape(&sample_token()).is_ok());
        let mut uppercase = sample_token();
        uppercase.make_ascii_uppercase();
        assert!(validate_token_shape(&uppercase).is_err());
        assert!(validate_token_shape(&sample_token()[..sample_token().len() - 2]).is_err());
        assert!(validate_token_shape("pp_session_abc").is_err());
    }
}
