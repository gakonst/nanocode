use crate::libc;
use std::{
    ffi::{CStr, CString, OsString},
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::ffi::{OsStrExt, OsStringExt},
    },
    path::{Path, PathBuf},
    time::Duration,
};

const USER_ENTRY_BUFFER_BYTES_DEFAULT: usize = 16_384;
const USER_ENTRY_BUFFER_BYTES_MAX: usize = 1_048_576;
const HOST_NAME_BUFFER_BYTES: usize = 256;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserEntry {
    pub name: String,
    pub home: PathBuf,
    pub shell: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemRelease {
    pub name: OsString,
    pub release: OsString,
}

fn checked(result: libc::c_int) -> io::Result<()> {
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[must_use]
pub fn user_id() -> u32 {
    unsafe { libc::getuid() }
}

#[must_use]
pub fn process_group() -> i32 {
    unsafe { libc::getpgrp() }
}

fn c_bytes(pointer: *const libc::c_char) -> Vec<u8> {
    if pointer.is_null() {
        return Vec::new();
    }
    unsafe { CStr::from_ptr(pointer) }.to_bytes().to_vec()
}

fn user_entry_buffer_bytes() -> usize {
    let configured = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    usize::try_from(configured)
        .ok()
        .filter(|bytes| *bytes > 0)
        .unwrap_or(USER_ENTRY_BUFFER_BYTES_DEFAULT)
}

pub fn user_entry(user_id: u32) -> io::Result<Option<UserEntry>> {
    let mut buffer: Vec<libc::c_char> = Vec::with_capacity(user_entry_buffer_bytes());
    loop {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                user_id,
                entry.as_mut_ptr(),
                buffer.as_mut_ptr(),
                buffer.capacity(),
                &raw mut found,
            )
        };
        if status == 0 {
            if found.is_null() {
                return Ok(None);
            }
            let entry = unsafe { entry.assume_init() };
            return Ok(Some(UserEntry {
                name: String::from_utf8_lossy(&c_bytes(entry.pw_name)).into_owned(),
                home: PathBuf::from(OsString::from_vec(c_bytes(entry.pw_dir))),
                shell: PathBuf::from(OsString::from_vec(c_bytes(entry.pw_shell))),
            }));
        }
        if status != libc::ERANGE {
            return Err(io::Error::from_raw_os_error(status));
        }
        let doubled = buffer.capacity().saturating_mul(2);
        if doubled > USER_ENTRY_BUFFER_BYTES_MAX {
            return Err(io::Error::from_raw_os_error(libc::ERANGE));
        }
        buffer.reserve_exact(doubled - buffer.len());
    }
}

fn field(bytes: &[libc::c_char]) -> OsString {
    OsString::from_vec(
        bytes
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| byte.to_ne_bytes()[0])
            .collect(),
    )
}

pub fn system_release() -> io::Result<SystemRelease> {
    let mut names = std::mem::MaybeUninit::<libc::utsname>::uninit();
    checked(unsafe { libc::uname(names.as_mut_ptr()) })?;
    let names = unsafe { names.assume_init() };
    Ok(SystemRelease {
        name: field(&names.sysname),
        release: field(&names.release),
    })
}

pub fn host_name() -> io::Result<OsString> {
    let mut buffer = vec![0_u8; HOST_NAME_BUFFER_BYTES];
    checked(unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) })?;
    let last = buffer.len() - 1;
    buffer[last] = 0;
    let length = buffer
        .iter()
        .position(|byte| *byte == 0)
        .expect("the host name buffer ends with a zero byte");
    buffer.truncate(length);
    Ok(OsString::from_vec(buffer))
}

pub fn signal_process(process_id: i32, signal: i32) -> io::Result<()> {
    checked(unsafe { libc::kill(process_id, signal) })
}

pub fn probe_process(process_id: i32) -> io::Result<()> {
    checked(unsafe { libc::kill(process_id, 0) })
}

pub fn signal_group(group_id: i32, signal: i32) -> io::Result<()> {
    checked(unsafe { libc::killpg(group_id, signal) })
}

pub fn sync_file(file: &File) -> io::Result<()> {
    checked(unsafe { libc::fsync(file.as_raw_fd()) })
}

#[cfg(target_vendor = "apple")]
pub fn barrier_sync_file(file: &File) -> io::Result<()> {
    checked(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_BARRIERFSYNC) })
}

pub fn available_bytes(path: &Path) -> io::Result<u64> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let mut statistics = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    checked(unsafe { libc::statvfs(path.as_ptr(), statistics.as_mut_ptr()) })?;
    let statistics = unsafe { statistics.assume_init() };
    #[allow(clippy::unnecessary_cast, clippy::useless_conversion)]
    let available = (statistics.f_bavail as u64).saturating_mul(u64::from(statistics.f_frsize));
    Ok(available)
}

pub fn redirect_output(file: &File) -> io::Result<()> {
    checked(unsafe { libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO) })?;
    checked(unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) })
}

pub fn thread_cpu_time() -> io::Result<Duration> {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    checked(unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, time.as_mut_ptr()) })?;
    let time = unsafe { time.assume_init() };
    let seconds =
        u64::try_from(time.tv_sec).map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    let nanoseconds =
        u64::try_from(time.tv_nsec).map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    assert!(nanoseconds < NANOSECONDS_PER_SECOND);
    Ok(Duration::new(
        seconds,
        u32::try_from(nanoseconds).expect("nanoseconds below one second fit in u32"),
    ))
}

#[cfg(target_os = "linux")]
pub fn forbid_core_dumps() -> io::Result<()> {
    checked(unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) })
}
