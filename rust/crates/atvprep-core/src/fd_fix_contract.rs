//! The manager pins an already scanned parent directory and sends only one leaf name plus a stable
//! identity; the target deliberately never accepts a path from the manager.
//!
//! Every operation on the leaf and its siblings goes through the pinned parent directory
//! (`openat`, `statat`, `linkat`, `renameat`, `unlinkat` with `NOFOLLOW`), so swapping a path
//! component underneath cannot redirect a write. An inherited descriptor is reopened through
//! `/dev/fd/<n>` (no raw-descriptor wrapping, no `unsafe`); the inherited descriptor itself stays
//! open and keeps being passed to child processes. Linux resolves `/dev/fd/<n>/<leaf>`;
//! the isolated macOS CLI enters the pinned directory before starting any tool threads.

use std::fmt;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, Stat, fstat, linkat, openat, renameat, statat, unlinkat,
};
use rustix::io::Errno;

use crate::prepare::paths::TEMP_PREFIX;

/// The inherited directory, the leaf name or the leaf identity is unsafe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsafeFixTarget(pub String);

impl fmt::Display for UnsafeFixTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UnsafeFixTarget {}

/// Enter an inherited directory in the isolated media CLI, before starting threads.
/// macOS fdesc paths support duplicating descriptors but cannot resolve directory children.
///
/// # Errors
/// Rejects invalid/non-directory descriptors or a failed descriptor-relative chdir.
#[cfg(target_os = "macos")]
pub fn enter_inherited_directory(fd: i32) -> Result<()> {
    if fd < 0 {
        return unsafe_target("parent descriptor is not a directory");
    }
    let directory = rustix::fs::open(
        format!("/dev/fd/{fd}"),
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| UnsafeFixTarget("parent descriptor is not a directory".to_owned()))?;
    let stat = fstat(&directory)
        .map_err(|_| UnsafeFixTarget("cannot inspect parent descriptor".to_owned()))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return unsafe_target("parent descriptor is not a directory");
    }
    rustix::process::fchdir(&directory)
        .map_err(|_| UnsafeFixTarget("cannot enter pinned media directory".to_owned()))
}

pub type Result<T> = std::result::Result<T, UnsafeFixTarget>;

/// Open an absolute library root without following any symlink component.
///
/// # Errors
/// Rejects relative roots, traversal, unavailable directories and symlinks.
pub fn open_library_root(root: &Path) -> Result<OwnedFd> {
    use std::path::Component;
    if !root.is_absolute() {
        return unsafe_target("library root must be absolute");
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = rustix::fs::open("/", flags, Mode::empty())
        .map_err(|_| UnsafeFixTarget("cannot open library root".to_owned()))?;
    for component in root.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = openat(&directory, name, flags, Mode::empty())
                    .map_err(|_| UnsafeFixTarget("cannot safely open library root".to_owned()))?;
            }
            _ => return unsafe_target("library root contains an unsafe component"),
        }
    }
    Ok(directory)
}

fn unsafe_target<T>(message: &str) -> Result<T> {
    Err(UnsafeFixTarget(message.to_owned()))
}

/// `(device, inode, ctime in nanoseconds)`: what pins a leaf.
type Identity = (i128, i128, i128);

/// Descriptors a child process needs because an argument is a `/dev/fd/N` path.
///
/// A descriptor that does not fit an `i32` is ignored. Rust's `Command` keeps an inherited
/// descriptor open in the child, so nothing has to be passed explicitly; this only names them.
#[must_use]
pub fn inherited_fds(command: &[String]) -> Vec<i32> {
    let mut found: Vec<i32> = command
        .iter()
        .filter_map(|argument| {
            let rest = argument.strip_prefix("/dev/fd/")?;
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let tail = &rest[digits..];
            // The Python pattern ends with `(?:/|$)`, and `$` also matches before a final newline.
            (digits > 0 && (tail.is_empty() || tail.starts_with('/') || tail == "\n"))
                .then(|| rest[..digits].parse().ok())?
        })
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

fn validate_leaf_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return unsafe_target("fix leaf must be one non-empty basename");
    }
    Ok(())
}

fn validate_temporary_leaf_name(name: &str) -> Result<()> {
    validate_leaf_name(name)?;
    if !name.starts_with(TEMP_PREFIX) {
        return unsafe_target("temporary output has an unexpected name");
    }
    Ok(())
}

fn ctime_ns(stat: &Stat) -> i128 {
    i128::from(stat.st_ctime) * 1_000_000_000 + i128::from(stat.st_ctime_nsec)
}

fn mtime_ns(stat: &Stat) -> i128 {
    i128::from(stat.st_mtime) * 1_000_000_000 + i128::from(stat.st_mtime_nsec)
}

fn is_directory(stat: &Stat) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::Directory
}

fn is_regular(stat: &Stat) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
}

fn directory_identity(stat: &Stat) -> (i128, i128) {
    (i128::from(stat.st_dev), i128::from(stat.st_ino))
}

/// Open `leaf` below `parent` without following a symlink and return its identity.
fn leaf_identity(parent: &OwnedFd, leaf: &str, open_failure: &str) -> Result<Identity> {
    let opened = openat(
        parent,
        leaf,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| UnsafeFixTarget(open_failure.to_owned()))?;
    let stat = fstat(&opened).map_err(|_| UnsafeFixTarget(open_failure.to_owned()))?;
    if !is_regular(&stat) {
        return unsafe_target("leaf is not a regular file");
    }
    Ok((
        i128::from(stat.st_dev),
        i128::from(stat.st_ino),
        ctime_ns(&stat),
    ))
}

/// One regular file whose final replacement is relative to a pinned parent directory.
#[derive(Debug, Clone)]
pub struct FdFixTarget {
    parent: Arc<OwnedFd>,
    parent_identity: (i128, i128),
    leaf_name: String,
    identity: Identity,
    /// The real directory; inherited targets use fdesc paths or a matching pinned macOS cwd.
    root: Option<PathBuf>,
    inherited: Option<i32>,
}

impl FdFixTarget {
    /// Duplicate the pinned parent for descriptor-based enumeration.
    ///
    /// # Errors
    /// Descriptor duplication fails.
    pub fn duplicate_parent(&self) -> Result<OwnedFd> {
        rustix::io::dup(&*self.parent)
            .map_err(|_| UnsafeFixTarget("cannot duplicate pinned directory".to_owned()))
    }

    /// Read metadata only while the pinned leaf still has the recorded identity.
    ///
    /// # Errors
    /// Rejects inaccessible or replaced leaves.
    pub fn metadata(&self) -> Result<Stat> {
        let file = openat(
            &*self.parent,
            self.leaf_name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| UnsafeFixTarget("cannot inspect pinned input".to_owned()))?;
        let stat =
            fstat(&file).map_err(|_| UnsafeFixTarget("cannot inspect pinned input".to_owned()))?;
        if !is_regular(&stat)
            || (
                i128::from(stat.st_dev),
                i128::from(stat.st_ino),
                ctime_ns(&stat),
            ) != self.identity
        {
            return unsafe_target("input identity changed");
        }
        Ok(stat)
    }

    /// Open the pinned leaf for read-only tools and retain it across tool execution.
    ///
    /// # Errors
    /// Rejects replaced inputs and failed descriptor configuration.
    pub fn open_for_tools(&self) -> Result<OwnedFd> {
        let file = openat(
            &*self.parent,
            self.leaf_name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| UnsafeFixTarget("cannot open pinned tool input".to_owned()))?;
        let stat = fstat(&file)
            .map_err(|_| UnsafeFixTarget("cannot inspect pinned tool input".to_owned()))?;
        if !is_regular(&stat)
            || (
                i128::from(stat.st_dev),
                i128::from(stat.st_ino),
                ctime_ns(&stat),
            ) != self.identity
        {
            return unsafe_target("tool input identity changed");
        }
        rustix::io::fcntl_setfd(&file, rustix::io::FdFlags::empty())
            .map_err(|_| UnsafeFixTarget("cannot preserve tool input descriptor".to_owned()))?;
        Ok(file)
    }

    /// Open the pinned leaf for in-place modification and keep it for the whole rewrite.
    ///
    /// Callers address the returned descriptor through `/dev/fd/N`, so every later open reaches the
    /// same inode even if the name is swapped for a symlink or another file meanwhile.
    ///
    /// # Errors
    /// Rejects replaced inputs and failed opens.
    pub fn open_for_write(&self) -> Result<OwnedFd> {
        let file = openat(
            &*self.parent,
            self.leaf_name.as_str(),
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| UnsafeFixTarget("cannot open pinned input for writing".to_owned()))?;
        let stat =
            fstat(&file).map_err(|_| UnsafeFixTarget("cannot inspect pinned input".to_owned()))?;
        if !is_regular(&stat)
            || (
                i128::from(stat.st_dev),
                i128::from(stat.st_ino),
                ctime_ns(&stat),
            ) != self.identity
        {
            return unsafe_target("input identity changed before writing");
        }
        Ok(file)
    }

    /// Pin a library-relative leaf without following symlinks in any path component.
    ///
    /// # Errors
    /// Rejects unsafe relative names, symlinked directories and non-regular leaves.
    pub fn from_library(root: &Path, relative: &str) -> Result<Self> {
        if !root.is_absolute()
            || relative.contains(['\\', '\0'])
            || relative
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return unsafe_target("media path must remain inside its library");
        }
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut directory = open_library_root(root)?;
        let mut parts = relative.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                return Self::pin(Arc::new(directory), part, None, None);
            }
            directory = openat(&directory, part, flags, Mode::empty())
                .map_err(|_| UnsafeFixTarget("cannot safely open media directory".to_owned()))?;
        }
        unsafe_target("media path must name a file")
    }

    fn pin(
        parent: Arc<OwnedFd>,
        leaf_name: &str,
        root: Option<PathBuf>,
        inherited: Option<i32>,
    ) -> Result<Self> {
        validate_leaf_name(leaf_name)?;
        let stat = fstat(&*parent)
            .map_err(|_| UnsafeFixTarget("parent descriptor could not be read".to_owned()))?;
        if !is_directory(&stat) {
            return unsafe_target("parent descriptor is not a directory");
        }
        let identity = leaf_identity(&parent, leaf_name, "could not safely open fix leaf")?;
        Ok(Self {
            parent_identity: directory_identity(&stat),
            parent,
            leaf_name: leaf_name.to_owned(),
            identity,
            root,
            inherited,
        })
    }

    /// Pin a standalone file: open its parent directory and keep real paths.
    ///
    /// The root is the parent as written (made absolute, never normalised), so `..` and symlinks
    /// are resolved by the OS and the root and the pinned descriptor agree.
    ///
    /// # Errors
    ///
    /// [`UnsafeFixTarget`] when the directory or the file cannot be opened safely.
    pub fn for_path(path: &Path) -> Result<Self> {
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|_| {
                    UnsafeFixTarget("could not safely open the parent directory".to_owned())
                })?
                .join(path)
        };
        let (Some(root), Some(leaf)) = (full.parent(), full.file_name().and_then(|n| n.to_str()))
        else {
            return unsafe_target("fix leaf must be one non-empty basename");
        };
        let directory = rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| UnsafeFixTarget("could not safely open the parent directory".to_owned()))?;
        Self::pin(Arc::new(directory), leaf, Some(root.to_path_buf()), None)
    }

    /// Pin the current regular leaf below an inherited directory descriptor.
    ///
    /// # Errors
    ///
    /// [`UnsafeFixTarget`] for an unusable descriptor, a bad leaf name or a leaf that is not a
    /// regular file.
    pub fn from_inherited(parent_fd: i32, leaf_name: &str) -> Result<Self> {
        validate_leaf_name(leaf_name)?;
        // No O_DIRECTORY here: macOS refuses it on /dev/fd/<n>; `pin` checks that it is a directory.
        let directory = rustix::fs::open(
            format!("/dev/fd/{parent_fd}"),
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| UnsafeFixTarget("parent descriptor is not a directory".to_owned()))?;
        // Standard descriptors may be replaced by media tools. Preserve a separate
        // inherited descriptor for their /dev/fd paths when the manager used stdin.
        let inherited = if parent_fd < 3 {
            rustix::io::fcntl_setfd(&directory, rustix::io::FdFlags::empty()).map_err(|_| {
                UnsafeFixTarget("cannot preserve media directory descriptor".to_owned())
            })?;
            directory.as_raw_fd()
        } else {
            parent_fd
        };
        Self::pin(Arc::new(directory), leaf_name, None, Some(inherited))
    }

    /// Accept a manager contract only when the inherited leaf still matches it.
    ///
    /// # Errors
    ///
    /// [`UnsafeFixTarget`] when the leaf identity changed.
    pub fn from_contract(
        parent_fd: i32,
        leaf_name: &str,
        device: i128,
        inode: i128,
        ctime_ns: i128,
    ) -> Result<Self> {
        let target = Self::from_inherited(parent_fd, leaf_name)?;
        if target.identity != (device, inode, ctime_ns) {
            return unsafe_target("leaf identity changed before fix started");
        }
        Ok(target)
    }

    #[must_use]
    pub fn leaf_name(&self) -> &str {
        &self.leaf_name
    }

    /// Device and inode of the pinned directory.
    #[must_use]
    pub fn parent_identity(&self) -> (i128, i128) {
        self.parent_identity
    }

    #[must_use]
    pub fn identity(&self) -> (i128, i128, i128) {
        self.identity
    }

    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The descriptor number child processes see: the inherited one, or this target's own.
    #[must_use]
    pub fn parent_fd_number(&self) -> i32 {
        self.inherited.unwrap_or_else(|| self.parent.as_raw_fd())
    }

    fn base(&self) -> PathBuf {
        if self.root.is_none()
            && cfg!(target_os = "macos")
            && rustix::fs::stat(".").is_ok_and(|stat| {
                (i128::from(stat.st_dev), i128::from(stat.st_ino)) == self.parent_identity
            })
        {
            return PathBuf::from(".");
        }
        self.root
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("/dev/fd/{}", self.parent_fd_number())))
    }

    /// A process-local path for the pinned leaf.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.base().join(&self.leaf_name)
    }

    /// A path for another leaf of the same pinned parent directory.
    ///
    /// # Errors
    ///
    /// [`UnsafeFixTarget`] for a name that is not one basename.
    pub fn sibling_path(&self, leaf_name: &str) -> Result<PathBuf> {
        validate_leaf_name(leaf_name)?;
        Ok(self.base().join(leaf_name))
    }

    /// Whether a leaf of that name exists in the pinned parent (symlinks are not followed).
    ///
    /// # Errors
    ///
    /// Any I/O error other than "not found", and a bad name as `InvalidInput`.
    pub fn has_sibling(&self, leaf_name: &str) -> std::io::Result<bool> {
        validate_leaf_name(leaf_name)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.0))?;
        match statat(&*self.parent, leaf_name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Pin an existing regular sibling under the same parent directory.
    ///
    /// # Errors
    ///
    /// [`UnsafeFixTarget`] when the parent changed or the sibling cannot be pinned.
    pub fn sibling(&self, leaf_name: &str) -> Result<Self> {
        self.check_parent()?;
        Self::pin(
            Arc::clone(&self.parent),
            leaf_name,
            self.root.clone(),
            self.inherited,
        )
    }

    fn check_parent(&self) -> Result<()> {
        match fstat(&*self.parent) {
            Ok(stat)
                if directory_identity(&stat) == self.parent_identity && is_directory(&stat) =>
            {
                Ok(())
            }
            _ => unsafe_target("parent directory identity changed"),
        }
    }

    /// Move a temporary output to a new name without ever overwriting an existing file.
    ///
    /// # Errors
    ///
    /// [`UnsafeFixTarget`] for bad names, a changed parent, a missing temporary or an existing output.
    pub fn publish_new(&self, temporary_leaf_name: &str, final_leaf_name: &str) -> Result<()> {
        self.publish_with(temporary_leaf_name, final_leaf_name, &|target, from, to| {
            linkat(&*target.parent, from, &*target.parent, to, AtFlags::empty())
        })
    }

    /// [`publish_new`](Self::publish_new) with the hard-link step replaceable (for tests).
    fn publish_with(
        &self,
        temporary_leaf_name: &str,
        final_leaf_name: &str,
        link: &dyn Fn(&Self, &str, &str) -> std::result::Result<(), Errno>,
    ) -> Result<()> {
        validate_temporary_leaf_name(temporary_leaf_name)?;
        validate_leaf_name(final_leaf_name)?;
        self.check_parent()?;
        let temporary = statat(
            &*self.parent,
            temporary_leaf_name,
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|_| UnsafeFixTarget("temporary output is unavailable".to_owned()))?;
        if !is_regular(&temporary) {
            return unsafe_target("temporary output is not a regular file");
        }
        match link(self, temporary_leaf_name, final_leaf_name) {
            Ok(()) => {
                // The output is in place; a temp that lingers must not turn a published file into a failure.
                let _ = unlinkat(&*self.parent, temporary_leaf_name, AtFlags::empty());
                Ok(())
            }
            Err(Errno::EXIST) => unsafe_target("output already exists"),
            Err(error) if ![Errno::PERM, Errno::NOTSUP, Errno::OPNOTSUPP].contains(&error) => {
                unsafe_target("could not publish the output")
            }
            Err(_) => {
                // No hard links on this filesystem: the best left is an absence check, then a rename.
                match statat(&*self.parent, final_leaf_name, AtFlags::SYMLINK_NOFOLLOW) {
                    Err(Errno::NOENT) => {}
                    Err(_) => return unsafe_target("could not check whether the output exists"),
                    Ok(_) => return unsafe_target("output already exists"),
                }
                renameat(
                    &*self.parent,
                    temporary_leaf_name,
                    &*self.parent,
                    final_leaf_name,
                )
                .map_err(|_| UnsafeFixTarget("could not publish the output".to_owned()))
            }
        }
    }

    /// Delete the pinned original once its output is verified, or say why it was kept.
    ///
    /// The original goes only when the output sits next to it, was written during this run and is
    /// not empty, and the original is still the file this target pinned. Returns `None` after the
    /// delete or when the original is already gone.
    #[must_use]
    pub fn remove_after_output(&self, output_leaf_name: &str, started_ns: i128) -> Option<String> {
        self.try_remove_after_output(output_leaf_name, started_ns)
            .err()
            .map(|reason| format!("the original was kept: {reason}"))
    }

    fn try_remove_after_output(
        &self,
        output: &str,
        started_ns: i128,
    ) -> std::result::Result<(), String> {
        validate_leaf_name(output).map_err(|e| e.0)?;
        self.check_parent().map_err(|e| e.0)?;
        if output == self.leaf_name {
            return Err("the output is the original itself".to_owned());
        }
        let written = statat(&*self.parent, output, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| format!("{output} was not found next to it"))?;
        if !is_regular(&written) || written.st_size <= 0 || mtime_ns(&written) < started_ns {
            return Err(format!("{output} was not written by this conversion"));
        }
        let source = match statat(
            &*self.parent,
            self.leaf_name.as_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(source) => source,
            Err(Errno::NOENT) => return Ok(()),
            Err(error) => return Err(std::io::Error::from(error).to_string()),
        };
        if directory_identity(&source) == directory_identity(&written) {
            return Err("the output is the original itself".to_owned());
        }
        if (
            i128::from(source.st_dev),
            i128::from(source.st_ino),
            ctime_ns(&source),
        ) != self.identity
        {
            return Err("it changed while it was converted".to_owned());
        }
        unlinkat(&*self.parent, self.leaf_name.as_str(), AtFlags::empty())
            .map_err(|e| std::io::Error::from(e).to_string())
    }

    /// Atomically replace the pinned leaf after an immediate identity check.
    ///
    /// # Errors
    ///
    /// [`UnsafeFixTarget`] when the parent or the leaf changed, or the temporary is unusable.
    pub fn replace_from(&self, temporary_leaf_name: &str) -> Result<()> {
        validate_temporary_leaf_name(temporary_leaf_name)?;
        self.check_parent()?;
        if leaf_identity(
            &self.parent,
            &self.leaf_name,
            "could not safely revalidate fix leaf",
        )? != self.identity
        {
            return unsafe_target("leaf identity changed before replacement");
        }
        let temporary = statat(
            &*self.parent,
            temporary_leaf_name,
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|_| UnsafeFixTarget("temporary output is unavailable".to_owned()))?;
        if !is_regular(&temporary) {
            return unsafe_target("temporary output is not a regular file");
        }
        renameat(
            &*self.parent,
            temporary_leaf_name,
            &*self.parent,
            self.leaf_name.as_str(),
        )
        .map_err(|_| UnsafeFixTarget("could not replace pinned leaf".to_owned()))
    }

    /// Fail closed unless the parent and the leaf still match the contract.
    ///
    /// # Errors
    ///
    /// [`UnsafeFixTarget`] when either changed.
    pub fn revalidate(&self) -> Result<()> {
        self.check_parent()?;
        if leaf_identity(
            &self.parent,
            &self.leaf_name,
            "could not safely revalidate fix leaf",
        )? != self.identity
        {
            return unsafe_target("leaf identity changed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        #[serde(default)]
        argv: Vec<String>,
        #[serde(default)]
        name: String,
    }

    #[test]
    fn parsing_and_validation_match_the_python_oracle() {
        for case in load_golden::<Input, Value>("fd_contract") {
            let actual = match case.input.kind.as_str() {
                "fds" => json!(inherited_fds(&case.input.argv)),
                "leaf" => json!(validate_leaf_name(&case.input.name).err().map(|e| e.0)),
                "temp" => json!(
                    validate_temporary_leaf_name(&case.input.name)
                        .err()
                        .map(|e| e.0)
                ),
                other => panic!("unknown kind {other}"),
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    const TEMP: &str = ".tmp_fixmeta_a.mp4";

    fn dir_with(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in files {
            std::fs::write(dir.path().join(name), bytes).unwrap();
        }
        dir
    }

    fn now_ns() -> i128 {
        i128::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
        .unwrap()
    }

    fn message<T>(result: std::result::Result<T, UnsafeFixTarget>) -> String {
        result.err().expect("an error").0
    }

    #[test]
    fn replace_from_rejects_a_leaf_that_was_swapped_and_replaces_an_unchanged_one() {
        let dir = dir_with(&[("movie.mp4", b"original"), (TEMP, b"fixed")]);
        let held = File::open(dir.path()).unwrap();
        let target = FdFixTarget::from_inherited(held.as_raw_fd(), "movie.mp4").unwrap();
        target.replace_from(TEMP).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("movie.mp4")).unwrap(),
            b"fixed"
        );
        assert!(!dir.path().join(TEMP).exists());

        // A leaf swapped after pinning is refused and nothing is replaced.
        std::fs::write(dir.path().join(TEMP), b"fixed again").unwrap();
        std::fs::write(dir.path().join("replacement.mp4"), b"attacker replacement").unwrap();
        std::fs::rename(
            dir.path().join("replacement.mp4"),
            dir.path().join("movie.mp4"),
        )
        .unwrap();
        let error = message(target.replace_from(TEMP));
        assert!(error.contains("identity changed"), "{error}");
        assert_eq!(
            std::fs::read(dir.path().join("movie.mp4")).unwrap(),
            b"attacker replacement"
        );
        assert_eq!(
            std::fs::read(dir.path().join(TEMP)).unwrap(),
            b"fixed again"
        );
    }

    #[test]
    fn non_leaf_names_are_rejected() {
        let dir = dir_with(&[]);
        let held = File::open(dir.path()).unwrap();
        for name in ["../movie.mp4", "nested/movie.mp4", ".", ""] {
            assert!(
                FdFixTarget::from_inherited(held.as_raw_fd(), name).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn for_path_pins_the_file_and_keeps_real_paths() {
        let dir = dir_with(&[("a.mp4", b"x")]);
        let target = FdFixTarget::for_path(&dir.path().join("a.mp4")).unwrap();
        assert_eq!(target.path(), dir.path().join("a.mp4"));
        assert_eq!(target.leaf_name(), "a.mp4");
        drop(target);
    }

    #[test]
    fn a_borrowed_descriptor_stays_open_when_the_target_is_dropped() {
        let dir = dir_with(&[("a.mp4", b"x")]);
        let held = File::open(dir.path()).unwrap();
        let target = FdFixTarget::from_inherited(held.as_raw_fd(), "a.mp4").unwrap();
        assert_eq!(
            target.path(),
            Path::new(&format!("/dev/fd/{}", held.as_raw_fd())).join("a.mp4")
        );
        drop(target);
        assert!(held.metadata().unwrap().is_dir());
    }

    #[test]
    fn publish_new_moves_the_temporary_into_place_and_never_overwrites() {
        let dir = dir_with(&[("a.mkv", b"src"), (TEMP, b"new")]);
        let target = FdFixTarget::for_path(&dir.path().join("a.mkv")).unwrap();
        target.publish_new(TEMP, "a.mp4").unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.mp4")).unwrap(), b"new");
        assert!(!dir.path().join(TEMP).exists());

        std::fs::write(dir.path().join(TEMP), b"newer").unwrap();
        let error = message(target.publish_new(TEMP, "a.mp4"));
        assert_eq!(error, "output already exists");
        assert_eq!(std::fs::read(dir.path().join("a.mp4")).unwrap(), b"new");
    }

    #[test]
    fn publish_new_rejects_names_without_the_temp_prefix_or_with_a_path() {
        let dir = dir_with(&[("a.mkv", b"src"), ("other.mp4", b"new")]);
        let target = FdFixTarget::for_path(&dir.path().join("a.mkv")).unwrap();
        assert!(message(target.publish_new("other.mp4", "a.mp4")).contains("unexpected name"));
        assert!(target.publish_new(".tmp_fixmeta_x", "../a.mp4").is_err());
        assert!(!dir.path().join("a.mp4").exists());
    }

    #[test]
    fn publish_falls_back_to_a_checked_rename_when_hard_links_are_unsupported() {
        let no_links = |_: &FdFixTarget, _: &str, _: &str| Err(rustix::io::Errno::PERM);
        let dir = dir_with(&[("a.mkv", b"src"), (TEMP, b"new")]);
        let target = FdFixTarget::for_path(&dir.path().join("a.mkv")).unwrap();
        target.publish_with(TEMP, "a.mp4", &no_links).unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.mp4")).unwrap(), b"new");
        assert!(!dir.path().join(TEMP).exists());

        // The fallback still refuses an existing final name, and leaves the temporary in place.
        std::fs::write(dir.path().join(TEMP), b"newer").unwrap();
        assert_eq!(
            message(target.publish_with(TEMP, "a.mp4", &no_links)),
            "output already exists"
        );
        assert_eq!(std::fs::read(dir.path().join("a.mp4")).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.path().join(TEMP)).unwrap(), b"newer");
    }

    #[test]
    fn any_other_link_error_is_a_failure_to_publish() {
        let broken = |_: &FdFixTarget, _: &str, _: &str| Err(rustix::io::Errno::IO);
        let dir = dir_with(&[("a.mkv", b"src"), (TEMP, b"new")]);
        let target = FdFixTarget::for_path(&dir.path().join("a.mkv")).unwrap();
        assert_eq!(
            message(target.publish_with(TEMP, "a.mp4", &broken)),
            "could not publish the output"
        );
        assert!(!dir.path().join("a.mp4").exists());
    }

    #[test]
    fn sibling_pins_an_existing_leaf_and_rejects_a_missing_one() {
        let dir = dir_with(&[("a.mkv", b"src"), ("a.mp4", b"out")]);
        let target = FdFixTarget::for_path(&dir.path().join("a.mkv")).unwrap();
        let sibling = target.sibling("a.mp4").unwrap();
        assert_eq!(sibling.path(), dir.path().join("a.mp4"));
        assert_eq!(sibling.parent_fd_number(), target.parent_fd_number());
        drop(sibling);
        assert!(target.has_sibling("a.mp4").unwrap());
        assert!(!target.has_sibling("b.mp4").unwrap());
        assert_eq!(
            target.sibling_path("b.mp4").unwrap(),
            dir.path().join("b.mp4")
        );
        assert!(target.sibling("b.mp4").is_err());
        target.revalidate().unwrap();
    }

    #[test]
    fn remove_after_output_deletes_a_verified_source() {
        let dir = dir_with(&[("a.mkv", b"src")]);
        let target = FdFixTarget::for_path(&dir.path().join("a.mkv")).unwrap();
        let started = now_ns() - 1_000_000_000;
        std::fs::write(dir.path().join("a.mp4"), b"out").unwrap();
        assert_eq!(target.remove_after_output("a.mp4", started), None);
        assert_eq!(
            target.remove_after_output("a.mp4", started),
            None,
            "already gone"
        );
        assert!(!dir.path().join("a.mkv").exists());
    }

    #[test]
    fn remove_after_output_keeps_the_source_and_says_why() {
        for case in ["missing", "empty", "older", "source-replaced"] {
            let dir = dir_with(&[("a.mkv", b"src")]);
            let target = FdFixTarget::for_path(&dir.path().join("a.mkv")).unwrap();
            let mut started = now_ns() - 1_000_000_000;
            if case != "missing" {
                std::fs::write(
                    dir.path().join("a.mp4"),
                    if case == "empty" {
                        &b""[..]
                    } else {
                        &b"out"[..]
                    },
                )
                .unwrap();
            }
            if case == "older" {
                started = now_ns() + 60_000_000_000;
            }
            if case == "source-replaced" {
                std::fs::write(dir.path().join("r.mkv"), b"other").unwrap();
                std::fs::rename(dir.path().join("r.mkv"), dir.path().join("a.mkv")).unwrap();
            }
            let expected = match case {
                "missing" => "the original was kept: a.mp4 was not found next to it",
                "source-replaced" => "the original was kept: it changed while it was converted",
                _ => "the original was kept: a.mp4 was not written by this conversion",
            };
            assert_eq!(
                target.remove_after_output("a.mp4", started).as_deref(),
                Some(expected),
                "{case}"
            );
            assert!(dir.path().join("a.mkv").exists(), "{case}");
        }
    }

    #[test]
    fn remove_after_output_refuses_when_the_output_is_the_original() {
        for form in ["same-name", "hard-link"] {
            let dir = dir_with(&[("a.mp4", b"src")]);
            let mut output = "a.mp4";
            if form == "hard-link" {
                std::fs::hard_link(dir.path().join("a.mp4"), dir.path().join("b.mp4")).unwrap();
                output = "b.mp4";
            }
            let target = FdFixTarget::for_path(&dir.path().join("a.mp4")).unwrap();
            let message = target.remove_after_output(output, now_ns() - 1_000_000_000);
            assert_eq!(
                message.as_deref(),
                Some("the original was kept: the output is the original itself"),
                "{form}"
            );
            assert!(dir.path().join("a.mp4").exists());
        }
    }

    #[test]
    fn remove_after_output_keeps_the_source_when_the_parent_identity_changed() {
        let dir = dir_with(&[("a.mkv", b"src"), ("a.mp4", b"out")]);
        let mut target = FdFixTarget::for_path(&dir.path().join("a.mkv")).unwrap();
        target.parent_identity = (0, 0);
        let message = target
            .remove_after_output("a.mp4", now_ns() - 1_000_000_000)
            .unwrap();
        assert!(message.starts_with("the original was kept:"), "{message}");
        assert!(dir.path().join("a.mkv").exists());
    }

    #[test]
    fn from_contract_accepts_only_a_matching_identity() {
        let dir = dir_with(&[("a.mp4", b"x")]);
        let held = File::open(dir.path()).unwrap();
        let pinned = FdFixTarget::from_inherited(held.as_raw_fd(), "a.mp4").unwrap();
        let (device, inode, ctime) = pinned.identity();
        FdFixTarget::from_contract(held.as_raw_fd(), "a.mp4", device, inode, ctime).unwrap();
        let error = message(FdFixTarget::from_contract(
            held.as_raw_fd(),
            "a.mp4",
            device,
            inode + 1,
            ctime,
        ));
        assert_eq!(error, "leaf identity changed before fix started");
    }

    #[test]
    fn a_symlink_leaf_is_refused() {
        let dir = dir_with(&[("real.mp4", b"x")]);
        symlink(dir.path().join("real.mp4"), dir.path().join("link.mp4")).unwrap();
        let held = File::open(dir.path()).unwrap();
        assert!(FdFixTarget::from_inherited(held.as_raw_fd(), "link.mp4").is_err());
        assert!(FdFixTarget::from_inherited(held.as_raw_fd(), "real.mp4").is_ok());
    }

    #[test]
    fn library_target_refuses_symlinked_components_and_traversal() {
        let directory = dir_with(&[("real.mp4", b"x")]);
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/film.mp4"), b"x").unwrap();
        symlink(root.join("nested"), root.join("link")).unwrap();
        assert!(FdFixTarget::from_library(&root, "nested/film.mp4").is_ok());
        for name in [
            "link/film.mp4",
            "../real.mp4",
            "nested/../real.mp4",
            "/real.mp4",
            "nested/./film.mp4",
        ] {
            assert!(FdFixTarget::from_library(&root, name).is_err(), "{name}");
        }
        symlink(root.join("real.mp4"), root.join("leaf.mp4")).unwrap();
        assert!(FdFixTarget::from_library(&root, "leaf.mp4").is_err());
    }

    #[test]
    fn for_path_keeps_the_root_lexical_and_the_pinned_directory_real() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("real").join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.path().join("real").join("a.mp4"), b"x").unwrap();
        symlink(&sub, dir.path().join("link")).unwrap();
        let target =
            FdFixTarget::for_path(&dir.path().join("link").join("..").join("a.mp4")).unwrap();
        assert_eq!(
            target.root(),
            Some(dir.path().join("link").join("..").as_path())
        );
        assert_eq!(std::fs::read(target.path()).unwrap(), b"x");
        target.revalidate().unwrap();
    }

    #[test]
    fn a_missing_directory_or_leaf_is_an_error_with_the_python_text() {
        let error = message(FdFixTarget::for_path(Path::new(
            "/definitely/not/here/a.mp4",
        )));
        assert_eq!(error, "could not safely open the parent directory");
        let dir = dir_with(&[]);
        let error = message(FdFixTarget::for_path(&dir.path().join("a.mp4")));
        assert_eq!(error, "could not safely open fix leaf");
    }

    #[test]
    fn the_write_descriptor_keeps_reaching_the_pinned_inode_after_the_name_is_swapped() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("movie.mp4");
        std::fs::write(&path, "abc").unwrap();
        let target = FdFixTarget::for_path(&path).unwrap();
        let pinned = target.open_for_write().unwrap();
        let moved = directory.path().join("moved.mp4");
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, "zzz").unwrap();
        let through = format!("/dev/fd/{}", pinned.as_raw_fd());
        std::fs::OpenOptions::new()
            .write(true)
            .open(through)
            .unwrap()
            .write_all(b"X")
            .unwrap();
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "Xbc");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "zzz");
        assert!(target.open_for_write().is_err());
    }
}
