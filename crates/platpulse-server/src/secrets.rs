//! Server pepper: the standalone secret file keying token digests
//! (design §12.5, §18.1).
//!
//! The pepper is created once by `init`, lives outside the database, and is
//! protected with a no-follow open-then-stat policy: `init` and `serve`
//! reject symlinks, non-regular files, files owned by another user, and
//! group/world-readable files instead of following or normalizing them.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use thiserror::Error;

/// Byte length of the pepper key (256-bit).
pub const PEPPER_BYTES: usize = 32;

/// Canonical pepper file content: 64 lowercase hex characters plus newline.
const PEPPER_HEX_CHARS: usize = PEPPER_BYTES * 2;

/// A 256-bit Server secret loaded from the pepper file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pepper([u8; PEPPER_BYTES]);

impl Pepper {
    /// HMAC-SHA-256 of `message` keyed with this pepper (design §12.5).
    pub fn hmac_digest(&self, message: &[u8]) -> [u8; PEPPER_BYTES] {
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(&self.0).expect("HMAC accepts any key length");
        mac.update(message);
        mac.finalize().into_bytes().into()
    }
}

#[derive(Debug, Error)]
pub enum PepperError {
    #[error("pepper file already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("failed to create pepper file {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open pepper file {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "pepper file {path} must be a regular file owned by the server user with no group or world access; run `platpulse-server init` as the dedicated server user"
    )]
    Unsafe { path: PathBuf },
    #[error("pepper file {path} must contain exactly {PEPPER_HEX_CHARS} lowercase hex characters")]
    Malformed { path: PathBuf },
    #[error("failed to read pepper file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Create a new pepper file exclusively (never overwrites), with mode 0600
/// and a no-follow create. The content is a random 256-bit hex string.
pub fn create_pepper_file(path: &Path) -> Result<(), PepperError> {
    let mut bytes = [0u8; PEPPER_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let hex = encode_hex(&bytes);

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
                    PepperError::AlreadyExists(path.to_owned())
                } else {
                    PepperError::Create {
                        path: path.to_owned(),
                        source,
                    }
                }
            })?
    };
    write_all_and_sync(&file, format!("{hex}\n").as_bytes(), path)?;
    Ok(())
}

/// Load and validate the pepper file with the no-follow open-then-stat
/// policy from design §18.1.
pub fn load_pepper_file(path: &Path) -> Result<Pepper, PepperError> {
    #[cfg(unix)]
    let file = open_nofollow_readonly(path)?;
    #[cfg(not(unix))]
    let file = File::open(path).map_err(|source| PepperError::Open {
        path: path.to_owned(),
        source,
    })?;

    #[cfg(unix)]
    assert_safe_regular_file(&file, path)?;

    let mut content = String::new();
    file.take(PEPPER_HEX_CHARS as u64 + 8)
        .read_to_string(&mut content)
        .map_err(|source| PepperError::Read {
            path: path.to_owned(),
            source,
        })?;
    let hex = content.trim_end_matches(['\n', '\r']);
    let bytes = decode_hex(hex).ok_or_else(|| PepperError::Malformed {
        path: path.to_owned(),
    })?;
    Ok(Pepper(bytes))
}

/// Write the pepper bytes as lowercase hex.
pub fn encode_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn decode_hex(hex: &str) -> Option<[u8; PEPPER_BYTES]> {
    if hex.len() != PEPPER_HEX_CHARS {
        return None;
    }
    let mut bytes = [0u8; PEPPER_BYTES];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_digit(chunk[0])?;
        let low = hex_digit(chunk[1])?;
        bytes[index] = (high << 4) | low;
    }
    Some(bytes)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(unix)]
fn open_new_nofollow(path: &Path) -> Result<File, PepperError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                PepperError::AlreadyExists(path.to_owned())
            } else {
                PepperError::Create {
                    path: path.to_owned(),
                    source,
                }
            }
        })?;
    assert_safe_regular_file(&file, path)?;
    Ok(file)
}

#[cfg(unix)]
fn open_nofollow_readonly(path: &Path) -> Result<File, PepperError> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)
        .map_err(|source| PepperError::Open {
            path: path.to_owned(),
            source,
        })
}

/// Open-then-stat: verify the opened descriptor is a regular file owned by
/// the current user with no group/world access bits.
#[cfg(unix)]
fn assert_safe_regular_file(file: &File, path: &Path) -> Result<(), PepperError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|source| PepperError::Read {
        path: path.to_owned(),
        source,
    })?;
    let mode = metadata.mode();
    let is_regular = metadata.file_type().is_file();
    let owned_by_server_user = metadata.uid() == nix::unistd::geteuid().as_raw();
    let no_group_or_world_access = mode & 0o077 == 0;
    if !is_regular || !owned_by_server_user || !no_group_or_world_access {
        return Err(PepperError::Unsafe {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn write_all_and_sync(mut file: &File, bytes: &[u8], path: &Path) -> Result<(), PepperError> {
    file.write_all(bytes)
        .map_err(|source| PepperError::Create {
            path: path.to_owned(),
            source,
        })?;
    file.sync_all().map_err(|source| PepperError::Create {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn create_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("server-pepper");
        create_pepper_file(&path).unwrap();
        let pepper = load_pepper_file(&path).unwrap();
        assert_eq!(pepper.hmac_digest(b"a"), pepper.hmac_digest(b"a"));
        assert_ne!(
            pepper.hmac_digest(b"a"),
            pepper.hmac_digest(b"b"),
            "digest must depend on the message"
        );
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn create_refuses_to_overwrite() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("server-pepper");
        create_pepper_file(&path).unwrap();
        let error = create_pepper_file(&path).unwrap_err();
        assert!(matches!(error, PepperError::AlreadyExists { .. }));
    }

    #[test]
    fn load_rejects_symlinked_pepper() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("real-pepper");
        create_pepper_file(&target).unwrap();
        let link = dir.path().join("server-pepper");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error = load_pepper_file(&link).unwrap_err();
        assert!(
            matches!(error, PepperError::Open { .. } | PepperError::Unsafe { .. }),
            "symlinked pepper must not be followed: {error}"
        );
    }

    #[test]
    fn load_rejects_group_readable_pepper() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("server-pepper");
        create_pepper_file(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let error = load_pepper_file(&path).unwrap_err();
        assert!(matches!(error, PepperError::Unsafe { .. }));
    }

    #[test]
    fn load_rejects_malformed_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("server-pepper");
        create_pepper_file(&path).unwrap();
        std::fs::write(&path, "not-hex").unwrap();

        let error = load_pepper_file(&path).unwrap_err();
        assert!(matches!(error, PepperError::Malformed { .. }));
    }

    #[test]
    fn load_rejects_directory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("server-pepper");
        std::fs::create_dir(&path).unwrap();
        let error = load_pepper_file(&path).unwrap_err();
        assert!(matches!(error, PepperError::Unsafe { .. }));
    }

    #[test]
    fn hex_encode_decode_roundtrip() {
        let bytes = [0xde, 0xad, 0xbe, 0xef, 0x00, 0xff, 0x01, 0x02];
        let encoded = encode_hex(&bytes);
        assert_eq!(encoded, "deadbeef00ff0102");
        let mut full = [0u8; PEPPER_BYTES];
        full[..8].copy_from_slice(&bytes);
        let hex64 = encode_hex(&full);
        let decoded = decode_hex(&hex64).unwrap();
        assert_eq!(&decoded[..8], &bytes);
        assert!(decode_hex("deadbeef").is_none(), "wrong length rejected");
        assert!(decode_hex(&"D".repeat(64)).is_none(), "uppercase rejected");
        assert!(decode_hex(&"z".repeat(64)).is_none(), "non-hex rejected");
    }
}
