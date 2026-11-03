//! Bounded local inputs, including an independently provisioned trusted release key.
use crate::{Error, Result};

/// An OS-released advisory lock serializing installations within one target directory.
pub struct InstallLock {
    _file: std::os::fd::OwnedFd,
}

fn protected_directory(directory: impl std::os::fd::AsFd) -> Result<()> {
    let stat = rustix::fs::fstat(directory)
        .map_err(|_| Error::Invalid("cannot inspect protected directory".to_owned()))?;
    if stat.st_mode & 0o022 != 0
        || (stat.st_uid != 0 && stat.st_uid != rustix::process::geteuid().as_raw())
    {
        return Err(Error::Invalid("release directory must be owned by the caller or root and protected from other writers".to_owned()));
    }
    Ok(())
}

/// Lock the pinned installation directory without following a lock-file symlink.
///
/// # Errors
/// Rejects unsafe lock files or another active installer.
pub fn installation_lock(destination: &Path) -> Result<InstallLock> {
    directory_lock(destination, ".atvprep-install.lock")
}

/// Serialize release windows in a protected database directory.
///
/// # Errors
/// Rejects unsafe lock files or another active release window.
pub fn window_lock(database: &Path) -> Result<InstallLock> {
    directory_lock(database, ".atvprep-window.lock")
}

/// Serialize offline schema migration processes.
///
/// # Errors
/// Rejects unsafe lock files or another active migration.
pub fn migration_lock(database: &Path) -> Result<InstallLock> {
    directory_lock(database, ".atvprep-migrate.lock")
}

fn directory_lock(destination: &Path, name: &str) -> Result<InstallLock> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|_| Error::Invalid("cannot open installation directory".to_owned()))?;
    let directory = atvprep_core::fd_fix_contract::open_library_root(&parent)
        .map_err(|_| Error::Invalid("cannot safely open installation directory".to_owned()))?;
    protected_directory(&directory)?;
    let file = rustix::fs::openat(
        &directory,
        name,
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .map_err(|_| Error::Invalid("cannot open installation lock".to_owned()))?;
    let stat = rustix::fs::fstat(&file)
        .map_err(|_| Error::Invalid("cannot inspect installation lock".to_owned()))?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_mode & 0o022 != 0
        || (stat.st_uid != 0 && stat.st_uid != rustix::process::geteuid().as_raw())
    {
        return Err(Error::Invalid(
            "installation lock is not protected".to_owned(),
        ));
    }
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| Error::Invalid("another installation is in progress".to_owned()))?;
    Ok(InstallLock { _file: file })
}
use std::{io::Read, path::Path};

/// Read a regular local file without following its final symlink.
///
/// # Errors
/// Rejects unsafe paths, non-regular files and data above the given bound.
pub fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    read(path, limit, false)
}

/// Load a public key owned by root or the caller and protected from other writers.
///
/// # Errors
/// Rejects unsafe key files, permissions, ownership and invalid UTF-8.
pub fn trusted_key(path: &Path) -> Result<String> {
    String::from_utf8(read(path, 4096, true)?)
        .map_err(|_| Error::Invalid("trusted release key is not valid UTF-8".to_owned()))
}

fn read(path: &Path, limit: u64, protected: bool) -> Result<Vec<u8>> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|_| Error::Invalid("cannot open local release directory".to_owned()))?;
    let leaf = path
        .file_name()
        .ok_or_else(|| Error::Invalid("local release path must name a file".to_owned()))?;
    let parent = atvprep_core::fd_fix_contract::open_library_root(&parent)
        .map_err(|_| Error::Invalid("cannot safely open local release directory".to_owned()))?;
    if protected {
        protected_directory(&parent)?;
    }
    let file = rustix::fs::openat(
        &parent,
        leaf,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| Error::Invalid("cannot safely open local release file".to_owned()))?;
    let metadata = rustix::fs::fstat(&file)
        .map_err(|_| Error::Invalid("cannot inspect local release file".to_owned()))?;
    if rustix::fs::FileType::from_raw_mode(metadata.st_mode) != rustix::fs::FileType::RegularFile
        || metadata.st_size < 0
        || u64::try_from(metadata.st_size).unwrap_or(u64::MAX) > limit
    {
        return Err(Error::Invalid(
            "local release input is not a bounded regular file".to_owned(),
        ));
    }
    if protected
        && ((metadata.st_uid != 0 && metadata.st_uid != rustix::process::geteuid().as_raw())
            || metadata.st_mode & 0o022 != 0)
    {
        return Err(Error::Invalid("trusted release key must be owned by the caller or root and protected from other writers".to_owned()));
    }
    let mut body = Vec::new();
    std::fs::File::from(file)
        .take(limit.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|_| Error::Invalid("cannot read local release file".to_owned()))?;
    if u64::try_from(body.len()).unwrap_or(u64::MAX) > limit {
        return Err(Error::Invalid(
            "local release input exceeds size limit".to_owned(),
        ));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn keys_are_bounded_protected_regular_files_and_install_locks_release() {
        let directory = tempfile::tempdir().unwrap();
        let key = directory.path().join("key");
        std::fs::write(&key, "public key").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(trusted_key(&key).unwrap(), "public key");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(trusted_key(&key).is_err());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = directory.path().join("link");
        symlink(&key, &link).unwrap();
        assert!(trusted_key(&link).is_err());
        std::fs::write(&key, vec![b'x'; 4097]).unwrap();
        assert!(trusted_key(&key).is_err());
        let destination = directory.path().join("atvprep");
        let lock = installation_lock(&destination).unwrap();
        assert!(installation_lock(&destination).is_err());
        drop(lock);
        assert!(installation_lock(&destination).is_ok());
    }
}
