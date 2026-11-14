//! File system changes, the rest of `stat`, user and group names and the local time
//! zone for the Unix host (D1150).

use nanocode_sys::libc;
use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

use psh_engine::host::ExtraMetadata;

use super::path;

fn cstr(p: &[u8]) -> io::Result<CString> {
    CString::new(p).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

pub fn extra(m: &std::fs::Metadata) -> ExtraMetadata {
    let ns = |s: i64, n: i64| s.saturating_mul(1_000_000_000).saturating_add(n);
    let btime_ns = m.created().ok().and_then(|t| {
        let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        i64::try_from(d.as_nanos()).ok()
    });
    ExtraMetadata {
        atime_ns: ns(m.atime(), m.atime_nsec()),
        ctime_ns: ns(m.ctime(), m.ctime_nsec()),
        btime_ns,
        nlink: m.nlink(),
        blocks: m.blocks(),
        blksize: m.blksize(),
        rdev: m.rdev(),
    }
}

/// The process's umask, as the spawn code reads it.
fn process_umask() -> u32 {
    super::spawn::process_umask()
}

pub fn mkdir(p: &[u8], mode: u32) -> io::Result<()> {
    let mode = mode & 0o7777;
    std::fs::DirBuilder::new().mode(mode).create(path(p))?;
    // mkdir(2) applies the process's umask; the caller asked for these bits exactly.
    // Only fix what the umask took away, so a setgid bit inherited from the parent
    // stays as mkdir(2) leaves it.
    if mode & process_umask() != 0 || mode & 0o7000 != 0 {
        std::fs::set_permissions(path(p), std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub fn remove_file(p: &[u8]) -> io::Result<()> {
    std::fs::remove_file(path(p))
}

pub fn remove_dir(p: &[u8]) -> io::Result<()> {
    std::fs::remove_dir(path(p))
}

pub fn rename(from: &[u8], to: &[u8]) -> io::Result<()> {
    std::fs::rename(path(from), path(to))
}

pub fn symlink(target: &[u8], link: &[u8]) -> io::Result<()> {
    std::os::unix::fs::symlink(path(target), path(link))
}

pub fn hard_link(existing: &[u8], link: &[u8]) -> io::Result<()> {
    // link(2) does not follow a symlink on Linux and does on macOS; POSIX `ln` (and
    // GNU's default) link the symlink itself.
    let (a, b) = (cstr(existing)?, cstr(link)?);
    // SAFETY: two valid NUL-terminated paths; no other pointers.
    let rc = unsafe { libc::linkat(libc::AT_FDCWD, a.as_ptr(), libc::AT_FDCWD, b.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn read_link(p: &[u8]) -> io::Result<Vec<u8>> {
    std::fs::read_link(path(p)).map(|t| t.into_os_string().into_vec())
}

pub fn set_mode(p: &[u8], mode: u32) -> io::Result<()> {
    std::fs::set_permissions(path(p), std::fs::Permissions::from_mode(mode & 0o7777))
}

pub fn set_times(p: &[u8], atime: Option<i64>, mtime: Option<i64>, follow: bool) -> io::Result<()> {
    let ts = |t: Option<i64>| match t {
        Some(ns) => libc::timespec {
            tv_sec: ns.div_euclid(1_000_000_000) as _,
            tv_nsec: ns.rem_euclid(1_000_000_000) as _,
        },
        None => libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
    };
    let times = [ts(atime), ts(mtime)];
    let c = cstr(p)?;
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    // SAFETY: a valid NUL-terminated path and a two-element timespec array, as
    // utimensat expects; both outlive the call.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), flags) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// A buffer size for the `get*_r` calls, grown on `ERANGE`.
const NAME_BUF: usize = 1024;

pub fn user_name(uid: u32) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; NAME_BUF];
    loop {
        // SAFETY: an all-zero passwd is a valid value of the plain C struct.
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: `pw`, `buf` (with its true length) and `out` are valid for the call;
        // the strings it stores point into `buf`, which outlives their use below.
        let rc =
            unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut out) };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 4, 0);
            continue;
        }
        if rc != 0 || out.is_null() || pw.pw_name.is_null() {
            return None;
        }
        // SAFETY: getpwuid_r succeeded: pw_name is a NUL-terminated string in `buf`.
        return Some(unsafe { CStr::from_ptr(pw.pw_name) }.to_bytes().to_vec());
    }
}

pub fn group_name(gid: u32) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; NAME_BUF];
    loop {
        // SAFETY: an all-zero group is a valid value of the plain C struct.
        let mut gr: libc::group = unsafe { std::mem::zeroed() };
        let mut out: *mut libc::group = std::ptr::null_mut();
        // SAFETY: as in `user_name`.
        let rc =
            unsafe { libc::getgrgid_r(gid, &mut gr, buf.as_mut_ptr().cast(), buf.len(), &mut out) };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 4, 0);
            continue;
        }
        if rc != 0 || out.is_null() || gr.gr_name.is_null() {
            return None;
        }
        // SAFETY: getgrgid_r succeeded: gr_name is a NUL-terminated string in `buf`.
        return Some(unsafe { CStr::from_ptr(gr.gr_name) }.to_bytes().to_vec());
    }
}

pub fn utc_offset(epoch_secs: i64) -> i32 {
    let t = epoch_secs as _;
    // SAFETY: an all-zero tm is a valid value of the plain C struct.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointers to a time_t and a tm for the call's duration;
    // localtime_r is the thread-safe form.
    let r = unsafe { libc::localtime_r(&t, &mut tm) };
    if r.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn changes_and_queries() {
        let dir = std::env::temp_dir().join(format!("psh-os-fs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.as_os_str().as_bytes().to_vec();
        let p = |name: &str| {
            let mut v = d.clone();
            v.push(b'/');
            v.extend_from_slice(name.as_bytes());
            v
        };
        mkdir(&d, 0o700).unwrap();
        // Exactly the bits asked for, whatever the process's umask.
        mkdir(&p("a"), 0o777).unwrap();
        let m = std::fs::metadata(path(&p("a"))).unwrap();
        assert_eq!(m.permissions().mode() & 0o7777, 0o777);
        assert_eq!(
            mkdir(&p("a"), 0o755).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        std::fs::write(path(&p("f")), b"x").unwrap();
        symlink(b"f", &p("l")).unwrap();
        assert_eq!(read_link(&p("l")).unwrap(), b"f");
        assert!(read_link(&p("f")).is_err());
        hard_link(&p("l"), &p("hl")).unwrap();
        assert!(
            std::fs::symlink_metadata(path(&p("hl")))
                .unwrap()
                .is_symlink()
        );
        set_mode(&p("f"), 0o600).unwrap();
        set_times(
            &p("f"),
            Some(5 * 1_000_000_000),
            Some(1_000_000_000_500),
            true,
        )
        .unwrap();
        let x = extra(&std::fs::metadata(path(&p("f"))).unwrap());
        assert_eq!(x.atime_ns, 5_000_000_000);
        let m = std::fs::metadata(path(&p("f"))).unwrap();
        assert_eq!((m.mtime(), m.mtime_nsec()), (1000, 500));
        assert_eq!(m.permissions().mode() & 0o7777, 0o600);
        // Only the link's own times.
        set_times(&p("l"), None, Some(0), false).unwrap();
        assert_eq!(std::fs::metadata(path(&p("f"))).unwrap().mtime(), 1000);
        rename(&p("f"), &p("g")).unwrap();
        remove_file(&p("g")).unwrap();
        remove_file(&p("l")).unwrap();
        remove_file(&p("hl")).unwrap();
        remove_dir(&p("a")).unwrap();
        remove_dir(&d).unwrap();
        // SAFETY: plain syscalls without pointers.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        assert!(user_name(uid).is_some_and(|n| !n.is_empty()));
        assert!(group_name(gid).is_some());
        let _ = utc_offset(0);
    }
}
