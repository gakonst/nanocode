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

#[derive(Debug, thiserror::Error)]
pub enum PrivateError {
    #[error("{} must be a directory", .0.display())]
    NotDirectory(PathBuf),
    #[error("{} must be owned by you and owner-private (chmod 700)", .0.display())]
    OpenDirectory(PathBuf),
    #[error("{} must be a regular file", .0.display())]
    NotFile(PathBuf),
    #[error("{} must be owned by you and private (0600)", .0.display())]
    OpenFile(PathBuf),
    #[error("{}: {error}", .path.display())]
    At { path: PathBuf, error: io::Error },
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub fn ensure_dir(dir: &Path) -> Result<(), PrivateError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    check_dir(dir)
}

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

pub fn write_new(path: &Path, bytes: &[u8]) -> Result<(), PrivateError> {
    write_new_with(path, |file| file.write_all(bytes))
}

pub fn write_new_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), PrivateError> {
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
    write(&mut file)?;
    file.sync_all()?;
    Ok(())
}

pub fn replace(path: &Path, bytes: &[u8]) -> Result<(), PrivateError> {
    replace_with(path, |file| file.write_all(bytes))
}

pub fn replace_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), PrivateError> {
    let own = process::id();
    let temporary = temporary(path, own);
    for (stale, pid) in temporaries(path) {
        if pid != own && !alive(pid) {
            let _ = fs::remove_file(stale);
        }
    }
    let _ = fs::remove_file(&temporary);
    let replaced = write_new_with(&temporary, write).and_then(|()| {
        fs::rename(&temporary, path)?;
        Ok(())
    });
    if replaced.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    replaced?;
    Ok(())
}

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

pub fn append_output(path: &Path) -> Result<(), PrivateError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| PrivateError::At {
            path: path.into(),
            error,
        })?;
    nix::unistd::dup2_stdout(&file).map_err(io::Error::from)?;
    nix::unistd::dup2_stderr(&file).map_err(io::Error::from)?;
    Ok(())
}

#[derive(Debug)]
pub struct Lock {
    file: File,
}

impl Lock {
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

    pub fn lock(&self) -> io::Result<()> {
        self.file.lock()
    }

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
