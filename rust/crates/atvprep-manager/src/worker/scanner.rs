//! Safe, deterministic scanner traversal primitives.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

fn nanoseconds(value: i128) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| Error::Invalid("scan timestamp exceeds supported range".to_owned()))
}

const VIDEO_SUFFIXES: [&str; 6] = [".mp4", ".m4v", ".mov", ".mkv", ".avi", ".webm"];

/// Descriptor-observed input and its associated regular subtitle files.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SeenFile {
    pub relative_path: String,
    pub meta: crate::models::MediaMeta,
    pub sidecars: Vec<SidecarMeta>,
    pub unsettled: bool,
}

/// Observe one leaf and its sidecars through pinned descriptors.
///
/// # Errors
/// Rejects unsafe paths, replaced inputs and invalid metadata.
pub fn observe(
    root: &Path,
    relative: &str,
    settle_seconds: i64,
    now_micros: i64,
) -> Result<SeenFile> {
    use rustix::fs::{AtFlags, Dir, FileType, statat};
    let target = atvprep_core::fd_fix_contract::FdFixTarget::from_library(root, relative)
        .map_err(|_| Error::Invalid("cannot safely open scanned media".to_owned()))?;
    let metadata = target
        .metadata()
        .map_err(|_| Error::Invalid("scanned media changed".to_owned()))?;
    let parent = target
        .duplicate_parent()
        .map_err(|_| Error::Invalid("cannot enumerate media sidecars".to_owned()))?;
    let video = Path::new(relative);
    let stem = video.file_stem().and_then(|v| v.to_str()).unwrap_or("");
    let mut sidecars = Vec::new();
    let entries = Dir::read_from(&parent)
        .map_err(|_| Error::Invalid("cannot enumerate media sidecars".to_owned()))?;
    for entry in entries {
        let entry =
            entry.map_err(|_| Error::Invalid("cannot read media sidecar entry".to_owned()))?;
        let name = entry
            .file_name()
            .to_str()
            .map_err(|_| Error::Invalid("sidecar name is not valid UTF-8".to_owned()))?;
        let path = Path::new(name);
        let suffix = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(suffix.as_str(), "srt" | "ass" | "ssa" | "vtt")
            || !path
                .file_stem()
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.starts_with(stem))
        {
            continue;
        }
        let stat = statat(&parent, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| Error::Invalid("sidecar changed during scan".to_owned()))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            continue;
        }
        let relative_path = video
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(
                || name.to_owned(),
                |parent| format!("{}/{name}", parent.display()),
            );
        sidecars.push(SidecarMeta {
            relative_path,
            size: stat.st_size,
            mtime_ns: stat
                .st_mtime
                .saturating_mul(1_000_000_000)
                .saturating_add(nanoseconds(stat.st_mtime_nsec.into())?),
        });
    }
    sidecars.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let mtime = metadata
        .st_mtime
        .saturating_mul(1_000_000_000)
        .saturating_add(nanoseconds(metadata.st_mtime_nsec.into())?);
    let (device, inode, ctime) = target.identity();
    let number = |value: i128| {
        i64::try_from(value)
            .map_err(|_| Error::Invalid("scan identity exceeds supported range".to_owned()))
    };
    let meta = crate::models::MediaMeta {
        kind: "video".to_owned(),
        suffix: video
            .extension()
            .and_then(|v| v.to_str())
            .map_or_else(String::new, |v| format!(".{v}").to_ascii_lowercase()),
        size: metadata.st_size,
        mtime_ns: mtime,
        fingerprint: fingerprint(metadata.st_size, mtime),
        sidecar_fingerprint: sidecar_fingerprint(&sidecars),
        scanned_device: Some(number(device)?),
        scanned_inode: Some(number(inode)?),
        scanned_ctime_ns: Some(number(ctime)?),
        ..crate::models::MediaMeta::default()
    };
    Ok(SeenFile {
        relative_path: relative.to_owned(),
        meta,
        sidecars,
        unsettled: now_micros.saturating_mul(1000).saturating_sub(mtime)
            < settle_seconds.max(0).saturating_mul(1_000_000_000),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SidecarMeta {
    pub relative_path: String,
    pub size: i64,
    pub mtime_ns: i64,
}

/// Python-compatible fingerprint for one media file's scanner-visible identity.
#[must_use]
pub fn fingerprint(size: i64, mtime_ns: i64) -> String {
    format!("{size}:{mtime_ns}")
}

/// Stable fingerprint of sidecars belonging to one media file.
#[must_use]
pub fn sidecar_fingerprint(sidecars: &[SidecarMeta]) -> String {
    let mut entries = sidecars.to_vec();
    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    entries
        .into_iter()
        .map(|sidecar| {
            let name = Path::new(&sidecar.relative_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            format!("{name}:{}:{}", sidecar.size, sidecar.mtime_ns)
        })
        .collect::<Vec<_>>()
        .join("|")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenPath {
    pub path: PathBuf,
    pub relative_path: String,
}

/// Reject a non-absolute, symlinked or unavailable configured root.
///
/// # Errors
///
/// The root cannot be validated as a real directory.
pub fn validate_root(root: &Path) -> Result<()> {
    if !root.is_absolute()
        || root
            .symlink_metadata()
            .map_or(true, |m| m.file_type().is_symlink() || !m.is_dir())
    {
        return Err(Error::Invalid(format!(
            "scan root is unsafe or unavailable: {}",
            root.display()
        )));
    }
    let mut current = PathBuf::new();
    for component in root.components() {
        current.push(component);
        if current
            .symlink_metadata()
            .map_or(true, |m| m.file_type().is_symlink() || !m.is_dir())
        {
            return Err(Error::Invalid(format!(
                "scan root is unsafe or unavailable: {}",
                root.display()
            )));
        }
    }
    Ok(())
}

#[must_use]
pub fn is_temporary(name: &str) -> bool {
    name.to_ascii_lowercase().contains(".tmp")
}
#[must_use]
pub fn is_excluded(relative: &str, name: &str, excluded: &[String]) -> bool {
    excluded
        .iter()
        .any(|entry| entry == if entry.contains('/') { relative } else { name })
}

fn video(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        VIDEO_SUFFIXES
            .contains(&format!(".{}", extension.to_string_lossy().to_ascii_lowercase()).as_str())
    })
}

/// Walk a library without following symlinks or crossing mount points.
///
/// # Errors
///
/// The root or one traversed directory cannot be read.
pub fn walk(root: &Path, follow_mounts: bool, excluded: &[String]) -> Result<Vec<SeenPath>> {
    walk_checked(root, follow_mounts, excluded, || Ok(()))
}

/// Walk while periodically validating and renewing the scanner's ownership.
///
/// # Errors
/// Returns filesystem errors and failed ownership checks.
pub fn walk_checked(
    root: &Path,
    follow_mounts: bool,
    excluded: &[String],
    mut check: impl FnMut() -> Result<()>,
) -> Result<Vec<SeenPath>> {
    let mut found = Vec::new();
    visit_checked(root, follow_mounts, excluded, &mut check, |file| {
        found.push(file);
        Ok(())
    })?;
    found.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(found)
}

/// Visit leaves depth-first with at most one pair of descriptors per nesting level.
///
/// # Errors
/// Rejects unsafe entries, root drift, excessive depth and visitor failures.
pub fn visit_checked(
    root: &Path,
    follow_mounts: bool,
    excluded: &[String],
    mut check: impl FnMut() -> Result<()>,
    mut visit: impl FnMut(SeenPath) -> Result<()>,
) -> Result<()> {
    use atvprep_core::fd_fix_contract::open_library_root;
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, fstat, openat, statat};
    let root_fd = open_library_root(root)
        .map_err(|_| Error::Invalid("scan root is unsafe or unavailable".to_owned()))?;
    let root_stat =
        fstat(&root_fd).map_err(|_| Error::Invalid("cannot inspect scan root".to_owned()))?;
    let entries = Dir::read_from(&root_fd)
        .map_err(|_| Error::Invalid("cannot read scan directory".to_owned()))?;
    let mut pending = vec![(root_fd, entries, String::new())];
    while let Some((directory, entries, prefix)) = pending.last_mut() {
        check()?;
        let Some(entry) = entries.next() else {
            pending.pop();
            continue;
        };
        let entry =
            entry.map_err(|_| Error::Invalid("cannot read scan directory entry".to_owned()))?;
        let name = entry.file_name();
        let name_text = name
            .to_str()
            .map_err(|_| Error::Invalid("scan entry is not valid UTF-8".to_owned()))?;
        if matches!(name_text, "." | "..") {
            continue;
        }
        let relative = if prefix.is_empty() {
            name_text.to_owned()
        } else {
            format!("{prefix}/{name_text}")
        };
        let metadata = statat(&*directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| Error::Invalid("scan entry changed".to_owned()))?;
        match FileType::from_raw_mode(metadata.st_mode) {
            FileType::Directory => {
                if is_excluded(&relative, name_text, excluded) {
                    continue;
                }
                let child = openat(
                    &*directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| Error::Invalid("scan directory changed".to_owned()))?;
                let actual = fstat(&child)
                    .map_err(|_| Error::Invalid("cannot inspect scan directory".to_owned()))?;
                if (actual.st_dev, actual.st_ino) != (metadata.st_dev, metadata.st_ino) {
                    return Err(Error::Invalid("scan directory changed".to_owned()));
                }
                if !follow_mounts && actual.st_dev != root_stat.st_dev {
                    continue;
                }
                let entries = Dir::read_from(&child)
                    .map_err(|_| Error::Invalid("cannot read scan directory".to_owned()))?;
                if pending.len() >= 256 {
                    return Err(Error::Invalid(
                        "scan directory nesting exceeds descriptor budget".to_owned(),
                    ));
                }
                pending.push((child, entries, relative));
            }
            FileType::RegularFile if video(Path::new(name_text)) && !is_temporary(name_text) => {
                visit(SeenPath {
                    path: root.join(&relative),
                    relative_path: relative,
                })?;
            }
            _ => {}
        }
    }
    let current =
        open_library_root(root).map_err(|_| Error::Invalid("scan root changed".to_owned()))?;
    let current =
        fstat(&current).map_err(|_| Error::Invalid("cannot inspect scan root".to_owned()))?;
    if (current.st_dev, current.st_ino) != (root_stat.st_dev, root_stat.st_ino) {
        return Err(Error::Invalid("scan root changed".to_owned()));
    }

    check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn wide_trees_have_depth_bounded_open_descriptors() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        for index in 0..512 {
            let child = root.join(format!("directory-{index}"));
            fs::create_dir(&child).unwrap();
            fs::write(child.join("film.mp4"), b"media").unwrap();
        }
        let descriptors = || fs::read_dir("/dev/fd").unwrap().count();
        let before = descriptors();
        let mut seen = 0;
        let mut maximum = before;
        visit_checked(
            &root,
            false,
            &[],
            || Ok(()),
            |_| {
                seen += 1;
                maximum = maximum.max(descriptors());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, 512);
        assert!(maximum < before + 128, "descriptors: {before} -> {maximum}");
    }

    #[test]
    fn replaced_root_and_interrupted_visitor_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let root = parent.join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("film.mp4"), b"original").unwrap();
        let mut replaced = false;
        let error = visit_checked(
            &root,
            false,
            &[],
            || {
                if !replaced {
                    fs::rename(&root, parent.join("old")).unwrap();
                    fs::create_dir(&root).unwrap();
                    fs::write(root.join("film.mp4"), b"replacement").unwrap();
                    replaced = true;
                }
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("root changed"));
        assert_eq!(fs::read(root.join("film.mp4")).unwrap(), b"replacement");
        let mut calls = 0;
        assert!(
            visit_checked(
                &root,
                false,
                &[],
                || Ok(()),
                |_| {
                    calls += 1;
                    Err(Error::Invalid("stopped".to_owned()))
                }
            )
            .is_err()
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn observation_captures_identity_sidecars_and_settle_state() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        for name in ["film.webm", "film.en.srt", "film.de.ass", "other.srt"] {
            fs::write(root.join(name), b"x").unwrap();
        }
        symlink(root.join("other.srt"), root.join("film.fr.vtt")).unwrap();
        let seen = observe(&root, "film.webm", 120, i64::MAX / 1000).unwrap();
        assert_eq!(seen.meta.suffix, ".webm");
        assert_eq!(seen.meta.size, 1);
        assert_eq!(
            seen.sidecars
                .iter()
                .map(|sidecar| sidecar.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["film.de.ass", "film.en.srt"]
        );
        assert!(!seen.unsettled);
        let fresh = observe(&root, "film.webm", 120, 0).unwrap();
        assert!(fresh.unsettled);
        assert_eq!(walk(&root, false, &[]).unwrap().len(), 1);
    }

    #[test]
    fn walk_never_follows_symlinked_files_or_directories() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("outside.mp4"), b"x").unwrap();
        fs::create_dir(root.path().join("inside")).unwrap();
        fs::write(root.path().join("inside/film.mp4"), b"x").unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();
        symlink(
            outside.path().join("outside.mp4"),
            root.path().join("link.mp4"),
        )
        .unwrap();
        let found = walk(&root.path().canonicalize().unwrap(), false, &[]).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|path| path.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["inside/film.mp4"]
        );
        let excluded = walk(
            &root.path().canonicalize().unwrap(),
            false,
            &["inside".to_owned()],
        )
        .unwrap();
        assert!(excluded.is_empty());
    }
    #[test]
    fn temp_and_excludes_match_python() {
        assert!(is_temporary("Film.TMP.mkv"));
        assert!(is_excluded("a/b", "b", &["b".to_owned()]));
        assert!(is_excluded("a/b", "b", &["a/b".to_owned()]));
    }
    #[test]
    fn fingerprints_are_stable_and_ordered() {
        assert_eq!(fingerprint(5, 7), "5:7");
        assert_eq!(
            sidecar_fingerprint(&[
                SidecarMeta {
                    relative_path: "b/x.srt".to_owned(),
                    size: 2,
                    mtime_ns: 3
                },
                SidecarMeta {
                    relative_path: "a/y.srt".to_owned(),
                    size: 4,
                    mtime_ns: 5
                }
            ]),
            "y.srt:4:5|x.srt:2:3"
        );
    }

    #[test]
    fn walk_skips_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["Film.mp4", ".tmp_fixmeta_Film.mp4", "Film.TMP.mkv"] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(
            walk(&dir.path().canonicalize().unwrap(), false, &[])
                .unwrap()
                .iter()
                .map(|p| p.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["Film.mp4"]
        );
    }
}
