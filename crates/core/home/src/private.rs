//! Files only their owner may read, as a host keeps a credential and its
//! state: a directory no one else may enter, files no one else may read,
//! written new or replaced whole, and lock files.

use nix::{
    errno::Errno,
    sys::signal::kill,
    unistd::{Pid, getuid},
};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process,
};

/// Why a private path was refused.
#[derive(Debug, thiserror::Error)]
pub enum PrivateError {
    #[error("{} must be a directory", .0.display())]
    NotDirectory(PathBuf),
    /// The directory is another user's, or others may enter it.
    #[error("{} must be owned by you and owner-private (chmod 700)", .0.display())]
    OpenDirectory(PathBuf),
    #[error("{} must be a regular file", .0.display())]
    NotFile(PathBuf),
    /// The file is another user's, or others may read it.
    #[error("{} must be owned by you and private (0600)", .0.display())]
    OpenFile(PathBuf),
    /// Opening or creating the path failed.
    #[error("{}: {error}", .path.display())]
    At { path: PathBuf, error: io::Error },
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Creates `dir` (and its parents) with mode 0700 if it is missing, then
/// [`check_dir`]s it.
pub fn ensure_dir(dir: &Path) -> Result<(), PrivateError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    check_dir(dir)
}

/// Whether `dir` is a directory, not a link, that the user owns and no one
/// else may enter.
pub fn check_dir(dir: &Path) -> Result<(), PrivateError> {
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(PrivateError::NotDirectory(dir.into()));
    }
    if meta.uid() != getuid().as_raw() || meta.mode() & 0o077 != 0 {
        return Err(PrivateError::OpenDirectory(dir.into()));
    }
    Ok(())
}

/// Opens `path` for reading if it is a regular file, not a link, that the
/// user owns and no one else may read or write.
pub fn open(path: &Path) -> Result<File, PrivateError> {
    let meta = fs::symlink_metadata(path).map_err(|error| PrivateError::At {
        path: path.into(),
        error,
    })?;
    if !meta.is_file() {
        return Err(PrivateError::NotFile(path.into()));
    }
    if meta.uid() != getuid().as_raw() || meta.permissions().mode() & 0o077 != 0 {
        return Err(PrivateError::OpenFile(path.into()));
    }
    Ok(File::open(path)?)
}

/// Writes `bytes` to a new 0600 file at `path`, never through a link and
/// never over an existing file, and syncs it.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<(), PrivateError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc_nofollow())
        .open(path)
        .map_err(|error| PrivateError::At {
            path: path.into(),
            error,
        })?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Replaces `path` whole with `bytes`, a 0600 file: written new as
/// `<name>.<pid>.tmp` beside it, then renamed over it, so a reader sees the
/// old file or the new one. The temporaries that processes that are gone
/// left are removed first.
pub fn replace(path: &Path, bytes: &[u8]) -> Result<(), PrivateError> {
    let own = process::id();
    let temporary = temporary(path, own);
    for (stale, pid) in temporaries(path) {
        if pid != own && !alive(pid) {
            let _ = fs::remove_file(stale);
        }
    }
    let _ = fs::remove_file(&temporary);
    write_new(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

/// The temporaries [`replace`] left beside `path`, with their processes' IDs.
pub fn temporaries(path: &Path) -> Vec<(PathBuf, u32)> {
    let (Some(dir), Some(name)) = (
        path.parent(),
        path.file_name().and_then(|name| name.to_str()),
    ) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let prefix = format!("{name}.");
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry
                .file_name()
                .to_str()?
                .strip_prefix(&prefix)?
                .strip_suffix(".tmp")?
                .parse()
                .ok()?;
            Some((entry.path(), pid))
        })
        .collect()
}

fn temporary(path: &Path, pid: u32) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{pid}.tmp"));
    path.with_file_name(name)
}

fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    matches!(kill(Pid::from_raw(pid), None), Ok(()) | Err(Errno::EPERM))
}

const fn libc_nofollow() -> i32 {
    libc::O_NOFOLLOW
}

/// A private lock file (0600, never through a link), taken exclusively as
/// `flock(2)` takes it, so it excludes any process that flocks the same file.
/// Dropping it lets the lock go.
#[derive(Debug)]
pub struct Lock {
    file: File,
}

impl Lock {
    /// Opens (creating) the lock file without taking it.
    pub fn open(path: &Path) -> Result<Self, PrivateError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc_nofollow())
            .open(path)?;
        open(path)?;
        Ok(Self { file })
    }

    /// Waits for the lock.
    pub fn lock(&self) -> io::Result<()> {
        self.file.lock()
    }

    /// Takes the lock unless another holds it, which fails with `EWOULDBLOCK`.
    pub fn try_lock(&self) -> io::Result<()> {
        match self.file.try_lock() {
            Ok(()) => Ok(()),
            Err(TryLockError::WouldBlock) => {
                Err(io::Error::from_raw_os_error(Errno::EWOULDBLOCK as i32))
            }
            Err(TryLockError::Error(error)) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_private_directory_is_created_owner_only_and_checked() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a/state");
        ensure_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o750)).unwrap();
        let open = check_dir(&dir).unwrap_err().to_string();
        assert!(
            open.ends_with("must be owned by you and owner-private (chmod 700)"),
            "{open}"
        );
        let file = root.path().join("file");
        fs::write(&file, "").unwrap();
        assert!(
            check_dir(&file)
                .unwrap_err()
                .to_string()
                .ends_with("must be a directory")
        );
        let link = root.path().join("link");
        std::os::unix::fs::symlink(root.path().join("a"), &link).unwrap();
        assert!(matches!(
            check_dir(&link),
            Err(PrivateError::NotDirectory(_))
        ));
    }

    #[test]
    fn private_files_are_written_new_and_read_only_while_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        write_new(&path, b"secret").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(io::read_to_string(open(&path).unwrap()).unwrap(), "secret");
        let again = write_new(&path, b"other").unwrap_err().to_string();
        assert!(
            again.starts_with(&format!("{}: ", path.display())),
            "{again}"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(matches!(open(&path), Err(PrivateError::OpenFile(_))));
        let missing = open(&dir.path().join("missing")).unwrap_err();
        assert!(matches!(missing, PrivateError::At { .. }));
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_new(&link, b"x").is_err(), "never through a link");
        assert!(!target.exists());
    }

    #[test]
    fn a_replace_is_whole_and_sweeps_gone_processes_temporaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("list.json");
        let stale = dir.path().join("list.json.999999999.tmp");
        fs::write(&stale, "[]").unwrap();
        replace(&path, b"[1]").unwrap();
        replace(&path, b"[2]").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "[2]");
        assert_eq!(mode(&path), 0o600);
        assert!(!stale.exists());
        assert!(temporaries(&path).is_empty());
        let own = dir.path().join(format!("list.json.{}.tmp", process::id()));
        fs::write(&own, "").unwrap();
        assert_eq!(temporaries(&path), [(own, process::id())]);
    }

    #[test]
    fn a_held_lock_refuses_another_holder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.lock");
        let first = Lock::open(&path).unwrap();
        first.try_lock().unwrap();
        assert_eq!(mode(&path), 0o600);
        let second = Lock::open(&path).unwrap();
        let refused = second.try_lock().unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(Errno::EWOULDBLOCK as i32));
        drop(first);
        second.lock().unwrap();
    }
}
