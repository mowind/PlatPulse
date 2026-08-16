//! Small, shared filesystem boundary checks for sensitive Server files.
//!
//! SQLite, Geo databases, and backup artifacts may contain current operational
//! data. They are opened with `O_NOFOLLOW` where the platform supports it and
//! are accepted only when they are regular files owned by the Server user
//! with no group/world write permissions. Sensitive directories use the
//! equivalent private-directory policy.

use std::path::Path;

#[cfg(unix)]
use std::fs::{File, OpenOptions};

#[cfg(unix)]
fn safe_file(file: &File) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect sensitive file".to_owned())?;
    if !metadata.file_type().is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(
            "sensitive file is not a private regular file owned by the Server user".to_owned(),
        );
    }
    Ok(())
}

/// Open an existing sensitive file without following a final symlink.
pub fn open_readonly(path: &Path) -> Result<std::fs::File, String> {
    validate_no_symlinked_ancestors(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .open(path)
            .map_err(|_| "cannot open sensitive file".to_owned())?;
        safe_file(&file)?;
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let file =
            std::fs::File::open(path).map_err(|_| "cannot open sensitive file".to_owned())?;
        let metadata = file
            .metadata()
            .map_err(|_| "cannot inspect sensitive file".to_owned())?;
        if !metadata.is_file() {
            return Err("sensitive path is not a regular file".to_owned());
        }
        Ok(file)
    }
}

/// Open an existing sensitive file for read/write validation.
pub fn open_readwrite(path: &Path) -> Result<std::fs::File, String> {
    validate_no_symlinked_ancestors(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .open(path)
            .map_err(|_| "cannot open sensitive file".to_owned())?;
        safe_file(&file)?;
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| "cannot open sensitive file".to_owned())?;
        let metadata = file
            .metadata()
            .map_err(|_| "cannot inspect sensitive file".to_owned())?;
        if !metadata.is_file() {
            return Err("sensitive path is not a regular file".to_owned());
        }
        Ok(file)
    }
}

/// Validate an existing regular file and its owner without requiring the
/// current permission bits to already be private. Initialization may use this
/// before repairing a file it owns; serving paths must use `validate_file`.
pub fn validate_owned_regular_file(path: &Path) -> Result<(), String> {
    validate_no_symlinked_ancestors(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .open(path)
            .map_err(|_| "cannot open sensitive file".to_owned())?;
        let metadata = file
            .metadata()
            .map_err(|_| "cannot inspect sensitive file".to_owned())?;
        if !metadata.file_type().is_file() || metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err("sensitive file is not a regular file owned by the Server user".to_owned());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| "cannot inspect sensitive file".to_owned())?;
        if !metadata.is_file() {
            return Err("sensitive path is not a regular file".to_owned());
        }
        Ok(())
    }
}
/// Reject a path whose existing ancestor is a symlink. The final path is
/// checked by the descriptor-based open functions, so this covers the
/// otherwise-followed parent components as well.
pub fn validate_no_symlinked_ancestors(path: &Path) -> Result<(), String> {
    let mut current = path.parent();
    while let Some(candidate) = current {
        if candidate.as_os_str().is_empty() {
            break;
        }
        if let Ok(metadata) = std::fs::symlink_metadata(candidate) {
            if metadata.file_type().is_symlink() {
                return Err("sensitive path has a symlinked ancestor".to_owned());
            }
        }
        current = candidate.parent();
    }
    Ok(())
}

/// Validate an existing file by descriptor, rather than path metadata.
pub fn validate_file(path: &Path) -> Result<(), String> {
    let _file = open_readonly(path)?;
    Ok(())
}
/// Validate an existing sensitive file and its containing directory.
pub fn validate_private_file(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    validate_private_directory(parent)?;
    validate_file(path)
}

/// Secure a file through a no-follow descriptor and validate the resulting
/// 0600 permissions.
pub fn secure_new_file(path: &Path) -> Result<(), String> {
    validate_no_symlinked_ancestors(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .open(path)
            .map_err(|_| "cannot open newly-created sensitive file".to_owned())?;
        let metadata = file
            .metadata()
            .map_err(|_| "cannot inspect newly-created sensitive file".to_owned())?;
        if !metadata.file_type().is_file() || metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err("newly-created sensitive path is not owned by the Server user".to_owned());
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "cannot secure newly-created sensitive file".to_owned())?;
        safe_file(&file)
    }
    #[cfg(not(unix))]
    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| "cannot open newly-created sensitive file".to_owned())?;
        if !file
            .metadata()
            .map_err(|_| "cannot inspect newly-created sensitive file".to_owned())?
            .is_file()
        {
            return Err("newly-created sensitive path is not a regular file".to_owned());
        }
        Ok(())
    }
}

/// Validate an existing private directory. A directory is not accepted when
/// it grants any group/world permissions.
pub fn validate_private_directory(path: &Path) -> Result<(), String> {
    validate_no_symlinked_ancestors(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(
                nix::fcntl::OFlag::O_DIRECTORY.bits() | nix::fcntl::OFlag::O_NOFOLLOW.bits(),
            )
            .open(path)
            .map_err(|_| "cannot open sensitive directory".to_owned())?;
        use std::os::unix::fs::MetadataExt;
        let metadata = directory
            .metadata()
            .map_err(|_| "cannot inspect sensitive directory".to_owned())?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.mode() & 0o022 != 0
        {
            return Err("sensitive directory is not owned by the Server user or is writable by another user".to_owned());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| "cannot inspect sensitive directory".to_owned())?;
        if !metadata.is_dir() {
            return Err("sensitive path is not a directory".to_owned());
        }
        Ok(())
    }
}

/// Create a private directory and validate it. Existing paths are never
/// repaired implicitly; an unsafe path fails closed.
pub fn ensure_private_directory(path: &Path) -> Result<(), String> {
    validate_no_symlinked_ancestors(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(_) => validate_private_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)
                .map_err(|_| "cannot create sensitive directory".to_owned())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| "cannot secure sensitive directory".to_owned())?;
            }
            validate_private_directory(path)
        }
        Err(_) => Err("cannot inspect sensitive directory".to_owned()),
    }
}

/// A filename stored in the backup registry must remain a plain base name.
pub fn is_safe_basename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|character| character == '/' || character == '\\' || character.is_control())
        && std::path::Path::new(value)
            .file_name()
            .is_some_and(|name| name == value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn rejects_symlink_and_group_access_for_sensitive_file() {
        let directory = tempdir().unwrap();
        let target = directory.path().join("target");
        std::fs::write(&target, b"secret").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_readonly(&link).is_err());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(open_readonly(&target).is_err());
    }

    #[test]
    fn rejects_symlinked_ancestors() {
        let directory = tempdir().unwrap();
        let real = directory.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(validate_no_symlinked_ancestors(&link.join("db.sqlite")).is_err());
        assert!(ensure_private_directory(&link.join("new")).is_err());
    }
    #[test]
    fn creates_private_directory_and_rejects_loose_permissions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("private");
        ensure_private_directory(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert!(validate_private_directory(&path).is_err());
    }

    #[test]
    fn private_file_requires_a_private_containing_directory() {
        let directory = tempdir().unwrap();
        let private = directory.path().join("private");
        std::fs::create_dir(&private).unwrap();
        let file = private.join("geo.mmdb");
        std::fs::write(&file, b"geo").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o777)).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(validate_private_file(&file).is_err());
        }
    }

    #[test]
    fn accepts_only_plain_basenames() {
        assert!(is_safe_basename("platpulse-a.db"));
        assert!(!is_safe_basename("../platpulse-a.db"));
        assert!(!is_safe_basename("/tmp/platpulse-a.db"));
        assert!(!is_safe_basename(""));
    }
}
