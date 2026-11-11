//! File system changes, the rest of `stat`, the user's name and the local time zone for
//! the Windows host (D1150, D1915).
//!
//! Windows differs from `unlink`/`rename`/`symlink` in ways a script notices, and the
//! host makes up for them: a read-only file is removed like any other (on Unix the
//! directory's permission decides, not the file's), a directory replaces an empty one,
//! a symlink or junction to a directory is removed without following it, and where
//! Windows refuses to make a symlink (neither Developer Mode nor administrator rights) a
//! directory gets a junction instead; a file then gets a clear error, never a copy.

use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use psh_engine::host::ExtraMetadata;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_WRITE_ATTRIBUTES,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::path;

fn ns(t: io::Result<std::time::SystemTime>) -> Option<i64> {
    let d = t.ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_nanos()).ok()
}

/// The rest of `stat`: access and creation times; Windows has no status-change time in
/// std's metadata, so `ctime` is the modification time.
pub(crate) fn extra(m: &std::fs::Metadata, links: u64) -> ExtraMetadata {
    let mtime = ns(m.modified()).unwrap_or(0);
    ExtraMetadata {
        atime_ns: ns(m.accessed()).unwrap_or(mtime),
        ctime_ns: mtime,
        btime_ns: ns(m.created()),
        nlink: links.max(1),
        blocks: m.len().div_ceil(512),
        blksize: 4096,
        rdev: 0,
    }
}

/// `mkdir`: the mode has no Windows meaning for a directory (the read-only attribute
/// does not stop changes inside one).
pub(crate) fn mkdir(p: &Path, _mode: u32) -> io::Result<()> {
    std::fs::create_dir(p)
}

fn is_dir_link(m: &std::fs::Metadata) -> bool {
    let a = m.file_attributes();
    a & FILE_ATTRIBUTE_REPARSE_POINT != 0 && a & FILE_ATTRIBUTE_DIRECTORY != 0
}

/// `unlink`: a file, a file symlink, or a symlink or junction to a directory (removed
/// as the link, never followed). A read-only file is removed too.
pub(crate) fn remove_file(p: &Path) -> io::Result<()> {
    let m = std::fs::symlink_metadata(p)?;
    if is_dir_link(&m) {
        return std::fs::remove_dir(p);
    }
    if m.is_dir() {
        return Err(io::ErrorKind::IsADirectory.into());
    }
    match std::fs::remove_file(p) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied && m.permissions().readonly() => {
            let mut perm = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            std::fs::set_permissions(p, perm)?;
            std::fs::remove_file(p)
        }
        r => r,
    }
}

/// `rename`: as Windows' replacing move, and a directory replaces an empty directory.
pub(crate) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Err(e)
            if std::fs::symlink_metadata(from).is_ok_and(|m| m.is_dir())
                && std::fs::symlink_metadata(to).is_ok_and(|m| m.is_dir() && !is_dir_link(&m)) =>
        {
            if std::fs::read_dir(to)?.next().is_some() {
                return Err(io::ErrorKind::DirectoryNotEmpty.into());
            }
            std::fs::remove_dir(to)?;
            std::fs::rename(from, to).map_err(|_| e)
        }
        r => r,
    }
}

/// A symlink `link` to `target` (engine bytes, stored with `\`): a directory symlink
/// when the target is a directory, else a file symlink. Refused for lack of the
/// privilege, a directory gets a junction (to the absolute target).
pub(crate) fn symlink(target: &[u8], link: &Path) -> io::Result<()> {
    let text = std::str::from_utf8(target)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target is not UTF-8"))?;
    let stored = if path::is_absolute(target) {
        path::native(target)?
    } else {
        std::path::PathBuf::from(text.replace('/', "\\"))
    };
    let resolved = if stored.is_absolute() {
        stored.clone()
    } else {
        link.parent().unwrap_or(Path::new(".")).join(&stored)
    };
    let dir = std::fs::metadata(&resolved).is_ok_and(|m| m.is_dir());
    let r = if dir {
        std::os::windows::fs::symlink_dir(&stored, link)
    } else {
        std::os::windows::fs::symlink_file(&stored, link)
    };
    match r {
        // ERROR_PRIVILEGE_NOT_HELD
        Err(e) if e.raw_os_error() == Some(1314) => {
            if dir {
                let abs = std::fs::canonicalize(&resolved)?;
                junction(&abs, link)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "symbolic links to files need Developer Mode or administrator rights",
                ))
            }
        }
        r => r,
    }
}

/// A junction (mount point) `link` to the directory `target` (absolute, may carry the
/// `\\?\` prefix).
fn junction(target: &Path, link: &Path) -> io::Result<()> {
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    let t = target.to_string_lossy();
    let t = t.strip_prefix(r"\\?\").unwrap_or(&t).to_string();
    let subst: Vec<u16> = format!(r"\??\{t}").encode_utf16().collect();
    let print: Vec<u16> = t.encode_utf16().collect();
    // The mount point's REPARSE_DATA_BUFFER: tag, data length, reserved, then the
    // substitute and print names' offsets and lengths, then both names NUL-terminated.
    let names_bytes = (subst.len() + 1 + print.len() + 1) * 2;
    let data_len = 8 + names_bytes;
    let mut buf: Vec<u8> = Vec::with_capacity(8 + data_len);
    buf.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buf.extend((data_len as u16).to_le_bytes());
    buf.extend(0u16.to_le_bytes());
    buf.extend(0u16.to_le_bytes());
    buf.extend(((subst.len() * 2) as u16).to_le_bytes());
    buf.extend((((subst.len() + 1) * 2) as u16).to_le_bytes());
    buf.extend(((print.len() * 2) as u16).to_le_bytes());
    for w in subst.iter().chain([&0]).chain(print.iter()).chain([&0]) {
        buf.extend(w.to_le_bytes());
    }
    std::fs::create_dir(link)?;
    let made = (|| {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(link)?;
        let mut returned = 0u32;
        // SAFETY: an open directory handle and a buffer valid for its length.
        let ok = unsafe {
            DeviceIoControl(
                f.as_raw_handle() as HANDLE,
                FSCTL_SET_REPARSE_POINT,
                buf.as_ptr().cast(),
                buf.len() as u32,
                std::ptr::null_mut(),
                0,
                &raw mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();
    if made.is_err() {
        let _ = std::fs::remove_dir(link);
    }
    made
}

/// A symlink's or junction's target as stored, `/`-separated. Not a link:
/// `InvalidInput`.
pub(crate) fn read_link(p: &Path) -> io::Result<Vec<u8>> {
    let m = std::fs::symlink_metadata(p)?;
    if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let t = std::fs::read_link(p)?;
    let s = t.to_string_lossy();
    let s = s.strip_prefix(r"\??\").unwrap_or(&s);
    Ok(path::engine_path(Path::new(s)))
}

/// `chmod`: the write bits are the read-only attribute (D1911); the rest has no
/// Windows meaning.
pub(crate) fn set_mode(p: &Path, mode: u32) -> io::Result<()> {
    let mut perm = std::fs::metadata(p)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(mode & 0o222 == 0);
    std::fs::set_permissions(p, perm)
}

pub(crate) fn set_times(
    p: &Path,
    atime_ns: Option<i64>,
    mtime_ns: Option<i64>,
    follow: bool,
) -> io::Result<()> {
    let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
    if !follow {
        flags |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    let f = std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(flags)
        .open(p)?;
    let at = |ns: i64| {
        if ns >= 0 {
            std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns as u64)
        } else {
            std::time::UNIX_EPOCH - std::time::Duration::from_nanos(ns.unsigned_abs())
        }
    };
    let mut times = std::fs::FileTimes::new();
    if let Some(a) = atime_ns {
        times = times.set_accessed(at(a));
    }
    if let Some(m) = mtime_ns {
        times = times.set_modified(at(m));
    }
    f.set_times(times)
}

/// The user's name (`USERNAME`).
pub(crate) fn user() -> Option<Vec<u8>> {
    std::env::var("USERNAME").ok().map(String::into_bytes)
}

/// The local offset east of UTC, in seconds, at `epoch_secs`, from the system's time
/// zone rules (daylight saving included).
pub(crate) fn utc_offset(epoch_secs: i64) -> i32 {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };
    // 100 ns ticks since 1601.
    let Some(ticks) = epoch_secs
        .checked_add(11_644_473_600)
        .and_then(|s| s.checked_mul(10_000_000))
        .filter(|t| *t >= 0)
    else {
        return 0;
    };
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    // SAFETY: plain data, filled by the calls below.
    let (mut utc, mut local): (SYSTEMTIME, SYSTEMTIME) = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointers to plain data.
    let ok = unsafe {
        FileTimeToSystemTime(&raw const ft, &raw mut utc) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &raw const utc, &raw mut local)
                != 0
    };
    if !ok {
        return 0;
    }
    // SAFETY: plain data.
    let mut lft: FILETIME = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointers to plain data.
    if unsafe { SystemTimeToFileTime(&raw const local, &raw mut lft) } == 0 {
        return 0;
    }
    let lticks = (i64::from(lft.dwHighDateTime) << 32) | i64::from(lft.dwLowDateTime);
    ((lticks - ticks) / 10_000_000) as i32
}
