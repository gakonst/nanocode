#[allow(unused_imports)]
use super::*;
pub use core::ffi::c_double;
pub type CCCryptorStatus = i32;
pub type CCRNGStatus = CCCryptorStatus;
pub type blkcnt_t = i64;
pub type blksize_t = i32;
pub type clock_t = c_ulong;
pub type clockid_t = c_uint;
pub type dev_t = i32;
pub type fsblkcnt_t = c_uint;
pub type fsfilcnt_t = c_uint;
pub type idtype_t = c_uint;
pub type ino_t = u64;
pub type mode_t = u16;
pub type nfds_t = c_uint;
pub type nl_item = c_int;
pub type nlink_t = u16;
pub type off_t = i64;
pub type posix_spawn_file_actions_t = *mut c_void;
pub type posix_spawnattr_t = *mut c_void;
pub type pthread_key_t = c_ulong;
pub type pthread_t = uintptr_t;
pub type rlim_t = u64;
pub type sa_family_t = u8;
pub type sigset_t = u32;
pub type speed_t = c_ulong;
pub type suseconds_t = i32;
pub type tcflag_t = c_ulong;
pub type time_t = c_long;
pub type wchar_t = i32;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct cmsghdr {
    pub cmsg_len: socklen_t,
    pub cmsg_level: c_int,
    pub cmsg_type: c_int,
}
impl ::core::default::Default for cmsghdr {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            cmsg_len: ::core::default::Default::default(),
            cmsg_level: ::core::default::Default::default(),
            cmsg_type: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[allow(clippy::manual_non_exhaustive)]
#[repr(C)]
pub struct fd_set {
    pub(super) fds_bits: [i32; FD_SETSIZE as usize / 32],
    pub(super) __non_exhaustive: (),
}
impl ::core::default::Default for fd_set {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            fds_bits: ::core::default::Default::default(),
            __non_exhaustive: (),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct in_addr {
    pub s_addr: in_addr_t,
}
impl ::core::default::Default for in_addr {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            s_addr: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
#[repr(packed(4))]
pub struct kevent {
    pub ident: uintptr_t,
    pub filter: i16,
    pub flags: u16,
    pub fflags: u32,
    pub data: intptr_t,
    pub udata: *mut c_void,
}
impl ::core::default::Default for kevent {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            ident: ::core::default::Default::default(),
            filter: ::core::default::Default::default(),
            flags: ::core::default::Default::default(),
            fflags: ::core::default::Default::default(),
            data: ::core::default::Default::default(),
            udata: ptr::null_mut(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct lconv {
    pub decimal_point: *mut c_char,
    pub thousands_sep: *mut c_char,
    pub grouping: *mut c_char,
    pub int_curr_symbol: *mut c_char,
    pub currency_symbol: *mut c_char,
    pub mon_decimal_point: *mut c_char,
    pub mon_thousands_sep: *mut c_char,
    pub mon_grouping: *mut c_char,
    pub positive_sign: *mut c_char,
    pub negative_sign: *mut c_char,
    pub int_frac_digits: c_char,
    pub frac_digits: c_char,
    pub p_cs_precedes: c_char,
    pub p_sep_by_space: c_char,
    pub n_cs_precedes: c_char,
    pub n_sep_by_space: c_char,
    pub p_sign_posn: c_char,
    pub n_sign_posn: c_char,
    pub int_p_cs_precedes: c_char,
    pub int_n_cs_precedes: c_char,
    pub int_p_sep_by_space: c_char,
    pub int_n_sep_by_space: c_char,
    pub int_p_sign_posn: c_char,
    pub int_n_sign_posn: c_char,
}
impl ::core::default::Default for lconv {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            decimal_point: ptr::null_mut(),
            thousands_sep: ptr::null_mut(),
            grouping: ptr::null_mut(),
            int_curr_symbol: ptr::null_mut(),
            currency_symbol: ptr::null_mut(),
            mon_decimal_point: ptr::null_mut(),
            mon_thousands_sep: ptr::null_mut(),
            mon_grouping: ptr::null_mut(),
            positive_sign: ptr::null_mut(),
            negative_sign: ptr::null_mut(),
            int_frac_digits: ::core::default::Default::default(),
            frac_digits: ::core::default::Default::default(),
            p_cs_precedes: ::core::default::Default::default(),
            p_sep_by_space: ::core::default::Default::default(),
            n_cs_precedes: ::core::default::Default::default(),
            n_sep_by_space: ::core::default::Default::default(),
            p_sign_posn: ::core::default::Default::default(),
            n_sign_posn: ::core::default::Default::default(),
            int_p_cs_precedes: ::core::default::Default::default(),
            int_n_cs_precedes: ::core::default::Default::default(),
            int_p_sep_by_space: ::core::default::Default::default(),
            int_n_sep_by_space: ::core::default::Default::default(),
            int_p_sign_posn: ::core::default::Default::default(),
            int_n_sign_posn: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct msghdr {
    pub msg_name: *mut c_void,
    pub msg_namelen: socklen_t,
    pub msg_iov: *mut iovec,
    pub msg_iovlen: c_int,
    pub msg_control: *mut c_void,
    pub msg_controllen: socklen_t,
    pub msg_flags: c_int,
}
impl ::core::default::Default for msghdr {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            msg_name: ptr::null_mut(),
            msg_namelen: ::core::default::Default::default(),
            msg_iov: ptr::null_mut(),
            msg_iovlen: ::core::default::Default::default(),
            msg_control: ptr::null_mut(),
            msg_controllen: ::core::default::Default::default(),
            msg_flags: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct passwd {
    pub pw_name: *mut c_char,
    pub pw_passwd: *mut c_char,
    pub pw_uid: uid_t,
    pub pw_gid: gid_t,
    pub pw_change: time_t,
    pub pw_class: *mut c_char,
    pub pw_gecos: *mut c_char,
    pub pw_dir: *mut c_char,
    pub pw_shell: *mut c_char,
    pub pw_expire: time_t,
}
impl ::core::default::Default for passwd {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            pw_name: ptr::null_mut(),
            pw_passwd: ptr::null_mut(),
            pw_uid: ::core::default::Default::default(),
            pw_gid: ::core::default::Default::default(),
            pw_change: ::core::default::Default::default(),
            pw_class: ptr::null_mut(),
            pw_gecos: ptr::null_mut(),
            pw_dir: ptr::null_mut(),
            pw_shell: ptr::null_mut(),
            pw_expire: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct pthread_attr_t {
    pub(super) __sig: c_long,
    pub(super) __opaque: [c_char; __PTHREAD_ATTR_SIZE__],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct rusage {
    pub ru_utime: timeval,
    pub ru_stime: timeval,
    pub ru_maxrss: c_long,
    pub ru_ixrss: c_long,
    pub ru_idrss: c_long,
    pub ru_isrss: c_long,
    pub ru_minflt: c_long,
    pub ru_majflt: c_long,
    pub ru_nswap: c_long,
    pub ru_inblock: c_long,
    pub ru_oublock: c_long,
    pub ru_msgsnd: c_long,
    pub ru_msgrcv: c_long,
    pub ru_nsignals: c_long,
    pub ru_nvcsw: c_long,
    pub ru_nivcsw: c_long,
}
impl ::core::default::Default for rusage {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            ru_utime: ::core::default::Default::default(),
            ru_stime: ::core::default::Default::default(),
            ru_maxrss: ::core::default::Default::default(),
            ru_ixrss: ::core::default::Default::default(),
            ru_idrss: ::core::default::Default::default(),
            ru_isrss: ::core::default::Default::default(),
            ru_minflt: ::core::default::Default::default(),
            ru_majflt: ::core::default::Default::default(),
            ru_nswap: ::core::default::Default::default(),
            ru_inblock: ::core::default::Default::default(),
            ru_oublock: ::core::default::Default::default(),
            ru_msgsnd: ::core::default::Default::default(),
            ru_msgrcv: ::core::default::Default::default(),
            ru_nsignals: ::core::default::Default::default(),
            ru_nvcsw: ::core::default::Default::default(),
            ru_nivcsw: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sched_param {
    pub sched_priority: c_int,
    pub(super) __opaque: [c_char; 4],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sigaction {
    pub sa_sigaction: sighandler_t,
    pub sa_mask: sigset_t,
    pub sa_flags: c_int,
}
impl ::core::default::Default for sigaction {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            sa_sigaction: ::core::default::Default::default(),
            sa_mask: ::core::default::Default::default(),
            sa_flags: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[allow(clippy::manual_non_exhaustive)]
#[repr(C)]
pub struct siginfo_t {
    pub si_signo: c_int,
    pub si_errno: c_int,
    pub si_code: c_int,
    pub si_pid: pid_t,
    pub si_uid: uid_t,
    pub si_status: c_int,
    pub si_addr: *mut c_void,
    pub(super) _pad: Padding<[usize; 9]>,
    pub(super) __non_exhaustive: (),
}
impl ::core::default::Default for siginfo_t {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            si_signo: ::core::default::Default::default(),
            si_errno: ::core::default::Default::default(),
            si_code: ::core::default::Default::default(),
            si_pid: ::core::default::Default::default(),
            si_uid: ::core::default::Default::default(),
            si_status: ::core::default::Default::default(),
            si_addr: ptr::null_mut(),
            _pad: ::core::default::Default::default(),
            __non_exhaustive: (),
        }
    }
}
impl siginfo_t {
    pub unsafe fn si_addr(&self) -> *mut c_void {
        unsafe { self.si_addr }
    }
    pub unsafe fn si_value(&self) -> sigval {
        unsafe {
            #[repr(C)]
            struct siginfo_timer {
                pub(super) _si_signo: c_int,
                pub(super) _si_errno: c_int,
                pub(super) _si_code: c_int,
                pub(super) _si_pid: pid_t,
                pub(super) _si_uid: uid_t,
                pub(super) _si_status: c_int,
                pub(super) _si_addr: *mut c_void,
                pub(super) si_value: sigval,
            }
            (*(self as *const siginfo_t).cast::<siginfo_timer>()).si_value
        }
    }
    pub unsafe fn si_pid(&self) -> pid_t {
        unsafe { self.si_pid }
    }
    pub unsafe fn si_uid(&self) -> uid_t {
        unsafe { self.si_uid }
    }
    pub unsafe fn si_status(&self) -> c_int {
        unsafe { self.si_status }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr {
    pub sa_len: u8,
    pub sa_family: sa_family_t,
    pub sa_data: [c_char; 14],
}
impl ::core::default::Default for sockaddr {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            sa_len: ::core::default::Default::default(),
            sa_family: ::core::default::Default::default(),
            sa_data: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr_in {
    pub sin_len: u8,
    pub sin_family: sa_family_t,
    pub sin_port: in_port_t,
    pub sin_addr: in_addr,
    pub sin_zero: [c_char; 8],
}
impl ::core::default::Default for sockaddr_in {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            sin_len: ::core::default::Default::default(),
            sin_family: ::core::default::Default::default(),
            sin_port: ::core::default::Default::default(),
            sin_addr: ::core::default::Default::default(),
            sin_zero: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr_in6 {
    pub sin6_len: u8,
    pub sin6_family: sa_family_t,
    pub sin6_port: in_port_t,
    pub sin6_flowinfo: u32,
    pub sin6_addr: in6_addr,
    pub sin6_scope_id: u32,
}
impl ::core::default::Default for sockaddr_in6 {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            sin6_len: ::core::default::Default::default(),
            sin6_family: ::core::default::Default::default(),
            sin6_port: ::core::default::Default::default(),
            sin6_flowinfo: ::core::default::Default::default(),
            sin6_addr: ::core::default::Default::default(),
            sin6_scope_id: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[allow(clippy::manual_non_exhaustive)]
#[repr(C)]
pub struct sockaddr_storage {
    pub ss_len: u8,
    pub ss_family: sa_family_t,
    pub(super) __ss_pad1: Padding<[u8; 6]>,
    pub(super) __ss_align: i64,
    pub(super) __ss_pad2: Padding<[u8; 112]>,
    pub(super) __non_exhaustive: (),
}
impl ::core::default::Default for sockaddr_storage {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            ss_len: ::core::default::Default::default(),
            ss_family: ::core::default::Default::default(),
            __ss_pad1: ::core::default::Default::default(),
            __ss_align: ::core::default::Default::default(),
            __ss_pad2: Padding::new([0; 112]),
            __non_exhaustive: (),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct stat {
    pub st_dev: dev_t,
    pub st_mode: mode_t,
    pub st_nlink: nlink_t,
    pub st_ino: ino_t,
    pub st_uid: uid_t,
    pub st_gid: gid_t,
    pub st_rdev: dev_t,
    pub st_atime: time_t,
    pub st_atime_nsec: c_long,
    pub st_mtime: time_t,
    pub st_mtime_nsec: c_long,
    pub st_ctime: time_t,
    pub st_ctime_nsec: c_long,
    pub st_birthtime: time_t,
    pub st_birthtime_nsec: c_long,
    pub st_size: off_t,
    pub st_blocks: blkcnt_t,
    pub st_blksize: blksize_t,
    pub st_flags: u32,
    pub st_gen: u32,
    pub st_lspare: i32,
    pub st_qspare: [i64; 2],
}
impl ::core::default::Default for stat {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            st_dev: ::core::default::Default::default(),
            st_mode: ::core::default::Default::default(),
            st_nlink: ::core::default::Default::default(),
            st_ino: ::core::default::Default::default(),
            st_uid: ::core::default::Default::default(),
            st_gid: ::core::default::Default::default(),
            st_rdev: ::core::default::Default::default(),
            st_atime: ::core::default::Default::default(),
            st_atime_nsec: ::core::default::Default::default(),
            st_mtime: ::core::default::Default::default(),
            st_mtime_nsec: ::core::default::Default::default(),
            st_ctime: ::core::default::Default::default(),
            st_ctime_nsec: ::core::default::Default::default(),
            st_birthtime: ::core::default::Default::default(),
            st_birthtime_nsec: ::core::default::Default::default(),
            st_size: ::core::default::Default::default(),
            st_blocks: ::core::default::Default::default(),
            st_blksize: ::core::default::Default::default(),
            st_flags: ::core::default::Default::default(),
            st_gen: ::core::default::Default::default(),
            st_lspare: ::core::default::Default::default(),
            st_qspare: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct statvfs {
    pub f_bsize: c_ulong,
    pub f_frsize: c_ulong,
    pub f_blocks: fsblkcnt_t,
    pub f_bfree: fsblkcnt_t,
    pub f_bavail: fsblkcnt_t,
    pub f_files: fsfilcnt_t,
    pub f_ffree: fsfilcnt_t,
    pub f_favail: fsfilcnt_t,
    pub f_fsid: c_ulong,
    pub f_flag: c_ulong,
    pub f_namemax: c_ulong,
}
impl ::core::default::Default for statvfs {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            f_bsize: ::core::default::Default::default(),
            f_frsize: ::core::default::Default::default(),
            f_blocks: ::core::default::Default::default(),
            f_bfree: ::core::default::Default::default(),
            f_bavail: ::core::default::Default::default(),
            f_files: ::core::default::Default::default(),
            f_ffree: ::core::default::Default::default(),
            f_favail: ::core::default::Default::default(),
            f_fsid: ::core::default::Default::default(),
            f_flag: ::core::default::Default::default(),
            f_namemax: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct termios {
    pub c_iflag: tcflag_t,
    pub c_oflag: tcflag_t,
    pub c_cflag: tcflag_t,
    pub c_lflag: tcflag_t,
    pub c_cc: [cc_t; NCCS],
    pub c_ispeed: speed_t,
    pub c_ospeed: speed_t,
}
impl ::core::default::Default for termios {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            c_iflag: ::core::default::Default::default(),
            c_oflag: ::core::default::Default::default(),
            c_cflag: ::core::default::Default::default(),
            c_lflag: ::core::default::Default::default(),
            c_cc: ::core::default::Default::default(),
            c_ispeed: ::core::default::Default::default(),
            c_ospeed: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct timespec {
    pub tv_sec: time_t,
    pub tv_nsec: c_long,
}
impl ::core::default::Default for timespec {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            tv_sec: ::core::default::Default::default(),
            tv_nsec: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct tm {
    pub tm_sec: c_int,
    pub tm_min: c_int,
    pub tm_hour: c_int,
    pub tm_mday: c_int,
    pub tm_mon: c_int,
    pub tm_year: c_int,
    pub tm_wday: c_int,
    pub tm_yday: c_int,
    pub tm_isdst: c_int,
    pub tm_gmtoff: c_long,
    pub tm_zone: *mut c_char,
}
impl ::core::default::Default for tm {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            tm_sec: ::core::default::Default::default(),
            tm_min: ::core::default::Default::default(),
            tm_hour: ::core::default::Default::default(),
            tm_mday: ::core::default::Default::default(),
            tm_mon: ::core::default::Default::default(),
            tm_year: ::core::default::Default::default(),
            tm_wday: ::core::default::Default::default(),
            tm_yday: ::core::default::Default::default(),
            tm_isdst: ::core::default::Default::default(),
            tm_gmtoff: ::core::default::Default::default(),
            tm_zone: ptr::null_mut(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct utsname {
    pub sysname: [c_char; 256],
    pub nodename: [c_char; 256],
    pub release: [c_char; 256],
    pub version: [c_char; 256],
    pub machine: [c_char; 256],
}
impl ::core::default::Default for utsname {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            sysname: [0; 256],
            nodename: [0; 256],
            release: [0; 256],
            version: [0; 256],
            machine: [0; 256],
        }
    }
}
pub const ABDAY_1: nl_item = 14;
pub const ABDAY_2: nl_item = 15;
pub const ABDAY_3: nl_item = 16;
pub const ABDAY_4: nl_item = 17;
pub const ABDAY_5: nl_item = 18;
pub const ABDAY_6: nl_item = 19;
pub const ABDAY_7: nl_item = 20;
pub const ABMON_1: nl_item = 33;
pub const ABMON_10: nl_item = 42;
pub const ABMON_11: nl_item = 43;
pub const ABMON_12: nl_item = 44;
pub const ABMON_2: nl_item = 34;
pub const ABMON_3: nl_item = 35;
pub const ABMON_4: nl_item = 36;
pub const ABMON_5: nl_item = 37;
pub const ABMON_6: nl_item = 38;
pub const ABMON_7: nl_item = 39;
pub const ABMON_8: nl_item = 40;
pub const ABMON_9: nl_item = 41;
pub const AF_INET6: c_int = 30;
pub const AF_LOCAL: c_int = 1;
pub const AF_UNIX: c_int = AF_LOCAL;
pub const AI_NUMERICHOST: c_int = 0x00000004;
pub const AI_PASSIVE: c_int = 0x00000001;
pub const ALT_DIGITS: nl_item = 49;
pub const AM_STR: nl_item = 5;
pub const AT_EACCESS: c_int = 0x0010;
pub const AT_FDCWD: c_int = -2;
pub const AT_REMOVEDIR: c_int = 0x0080;
pub const AT_SYMLINK_FOLLOW: c_int = 0x0040;
pub const AT_SYMLINK_NOFOLLOW: c_int = 0x0020;
pub const CLOCK_MONOTONIC: clockid_t = 6;
pub const CLOCK_MONOTONIC_RAW_APPROX: clockid_t = 5;
pub const CLOCK_PROCESS_CPUTIME_ID: clockid_t = 12;
pub const CLOCK_THREAD_CPUTIME_ID: clockid_t = 16;
pub const CLOCK_UPTIME_RAW: clockid_t = 8;
pub const CLOCK_UPTIME_RAW_APPROX: clockid_t = 9;
pub const CODESET: nl_item = 0;
pub const CRNCYSTR: nl_item = 56;
pub const CS8: tcflag_t = 0x00000300;
pub const CSIZE: tcflag_t = 0x00000300;
pub const DAY_1: nl_item = 7;
pub const DAY_2: nl_item = 8;
pub const DAY_3: nl_item = 9;
pub const DAY_4: nl_item = 10;
pub const DAY_5: nl_item = 11;
pub const DAY_6: nl_item = 12;
pub const DAY_7: nl_item = 13;
pub const D_FMT: nl_item = 2;
pub const D_T_FMT: nl_item = 1;
pub const EADDRINUSE: c_int = 48;
pub const EADDRNOTAVAIL: c_int = 49;
pub const EAFNOSUPPORT: c_int = 47;
pub const EAGAIN: c_int = 35;
pub const EALREADY: c_int = 37;
pub const EAUTH: c_int = 80;
pub const EBADARCH: c_int = 86;
pub const EBADEXEC: c_int = 85;
pub const EBADMACHO: c_int = 88;
pub const EBADMSG: c_int = 94;
pub const EBADRPC: c_int = 72;
pub const ECANCELED: c_int = 89;
pub const ECHONL: tcflag_t = 0x00000010;
pub const ECONNABORTED: c_int = 53;
pub const ECONNREFUSED: c_int = 61;
pub const ECONNRESET: c_int = 54;
pub const EDEADLK: c_int = 11;
pub const EDESTADDRREQ: c_int = 39;
pub const EDEVERR: c_int = 83;
pub const EDQUOT: c_int = 69;
pub const EFTYPE: c_int = 79;
pub const EHOSTDOWN: c_int = 64;
pub const EHOSTUNREACH: c_int = 65;
pub const EIDRM: c_int = 90;
pub const EILSEQ: c_int = 92;
pub const EINPROGRESS: c_int = 36;
pub const EISCONN: c_int = 56;
pub const ELOOP: c_int = 62;
pub const EMSGSIZE: c_int = 40;
pub const EMULTIHOP: c_int = 95;
pub const ENAMETOOLONG: c_int = 63;
pub const ENEEDAUTH: c_int = 81;
pub const ENETDOWN: c_int = 50;
pub const ENETRESET: c_int = 52;
pub const ENETUNREACH: c_int = 51;
pub const ENOATTR: c_int = 93;
pub const ENOBUFS: c_int = 55;
pub const ENODATA: c_int = 96;
pub const ENOLCK: c_int = 77;
pub const ENOLINK: c_int = 97;
pub const ENOMSG: c_int = 91;
pub const ENOPOLICY: c_int = 103;
pub const ENOPROTOOPT: c_int = 42;
pub const ENOSR: c_int = 98;
pub const ENOSTR: c_int = 99;
pub const ENOSYS: c_int = 78;
pub const ENOTCAPABLE: c_int = 107;
pub const ENOTCONN: c_int = 57;
pub const ENOTEMPTY: c_int = 66;
pub const ENOTRECOVERABLE: c_int = 104;
pub const ENOTSOCK: c_int = 38;
pub const ENOTSUP: c_int = 45;
pub const EOPNOTSUPP: c_int = 102;
pub const EOVERFLOW: c_int = 84;
pub const EOWNERDEAD: c_int = 105;
pub const EPFNOSUPPORT: c_int = 46;
pub const EPROCLIM: c_int = 67;
pub const EPROCUNAVAIL: c_int = 76;
pub const EPROGMISMATCH: c_int = 75;
pub const EPROGUNAVAIL: c_int = 74;
pub const EPROTO: c_int = 100;
pub const EPROTONOSUPPORT: c_int = 43;
pub const EPROTOTYPE: c_int = 41;
pub const EPWROFF: c_int = 82;
pub const EQFULL: c_int = 106;
pub const ERA: nl_item = 45;
pub const ERA_D_FMT: nl_item = 46;
pub const ERA_D_T_FMT: nl_item = 47;
pub const ERA_T_FMT: nl_item = 48;
pub const EREMOTE: c_int = 71;
pub const ERPCMISMATCH: c_int = 73;
pub const ESHLIBVERS: c_int = 87;
pub const ESHUTDOWN: c_int = 58;
pub const ESOCKTNOSUPPORT: c_int = 44;
pub const ESTALE: c_int = 70;
pub const ETIME: c_int = 101;
pub const ETIMEDOUT: c_int = 60;
pub const ETOOMANYREFS: c_int = 59;
pub const EUSERS: c_int = 68;
pub const EVFILT_READ: i16 = -1;
pub const EVFILT_USER: i16 = -10;
pub const EVFILT_WRITE: i16 = -2;
pub const EV_ADD: u16 = 0x1;
pub const EV_CLEAR: u16 = 0x20;
pub const EV_DELETE: u16 = 0x2;
pub const EV_EOF: u16 = 0x8000;
pub const EV_ERROR: u16 = 0x4000;
pub const EV_RECEIPT: u16 = 0x40;
pub const FIONBIO: c_ulong = 0x8004667e;
pub const F_BARRIERFSYNC: c_int = 85;
pub const F_DUPFD_CLOEXEC: c_int = 67;
pub const ICANON: tcflag_t = 0x00000100;
pub const IEXTEN: tcflag_t = 0x00000400;
pub(super) const IOCPARM_MASK: c_ulong = 0x1fff;
pub(super) const IOCPARM_MAX: c_ulong = IOCPARM_MASK + 1;
pub(super) const IOC_DIRMASK: u32 = 0xe0000000;
pub(super) const IOC_IN: u32 = 0x80000000;
pub(super) const IOC_OUT: u32 = 0x40000000;
pub(super) const IOC_VOID: u32 = 0x20000000;
pub const ISIG: tcflag_t = 0x00000080;
pub const ITIMER_PROF: c_int = 2;
pub const ITIMER_REAL: c_int = 0;
pub const ITIMER_VIRTUAL: c_int = 1;
pub const IXON: tcflag_t = 0x00000200;
pub const LC_ALL: c_int = 0;
pub const LC_COLLATE: c_int = 1;
pub const LC_CTYPE: c_int = 2;
pub const LC_MESSAGES: c_int = 6;
pub const LC_MONETARY: c_int = 3;
pub const LC_NUMERIC: c_int = 4;
pub const LC_TIME: c_int = 5;
pub const MADV_FREE_REUSABLE: c_int = 7;
pub const MAP_ANON: c_int = 0x1000;
pub const MAP_ANONYMOUS: c_int = MAP_ANON;
pub const MON_1: nl_item = 21;
pub const MON_10: nl_item = 30;
pub const MON_11: nl_item = 31;
pub const MON_12: nl_item = 32;
pub const MON_2: nl_item = 22;
pub const MON_3: nl_item = 23;
pub const MON_4: nl_item = 24;
pub const MON_5: nl_item = 25;
pub const MON_6: nl_item = 26;
pub const MON_7: nl_item = 27;
pub const MON_8: nl_item = 28;
pub const MON_9: nl_item = 29;
pub const NCCS: usize = 20;
pub const NOEXPR: nl_item = 53;
pub const NOTE_TRIGGER: u32 = 0x01000000;
pub const O_ACCMODE: c_int = 0x3;
pub const O_APPEND: c_int = 8;
pub const O_ASYNC: c_int = 0x40;
pub const O_CLOEXEC: c_int = 0x01000000;
pub const O_CREAT: c_int = 512;
pub const O_DIRECTORY: c_int = 0x00100000;
pub const O_DSYNC: c_int = 0x00400000;
pub const O_EVTONLY: c_int = 0x00008000;
pub const O_EXCL: c_int = 2048;
pub const O_EXEC: c_int = 0x40000000;
pub const O_EXLOCK: c_int = 0x20;
pub const O_FSYNC: c_int = O_SYNC;
pub const O_NDELAY: c_int = O_NONBLOCK;
pub const O_NOCTTY: c_int = 0x00020000;
pub const O_NOFOLLOW: c_int = 0x100;
pub const O_NOFOLLOW_ANY: c_int = 0x20000000;
pub const O_NONBLOCK: c_int = 0x4;
pub const O_SEARCH: c_int = O_EXEC | O_DIRECTORY;
pub const O_SHLOCK: c_int = 0x10;
pub const O_SYMLINK: c_int = 0x00200000;
pub const O_SYNC: c_int = 0x80;
pub const O_TRUNC: c_int = 1024;
pub const PARENB: tcflag_t = 0x00001000;
pub const PIPE_BUF: usize = 512;
pub const PM_STR: nl_item = 6;
pub const POSIX_SPAWN_CLOEXEC_DEFAULT: c_int = 0x4000;
pub const POSIX_SPAWN_SETPGROUP: c_int = 0x0002;
pub const POSIX_SPAWN_SETSIGDEF: c_int = 0x0004;
pub const POSIX_SPAWN_SETSIGMASK: c_int = 0x0008;
pub const PTHREAD_CREATE_DETACHED: c_int = 2;
pub const RADIXCHAR: nl_item = 50;
pub const RLIMIT_AS: c_int = 5;
pub const RLIMIT_CORE: c_int = 4;
pub const RLIMIT_CPU: c_int = 0;
pub const RLIMIT_DATA: c_int = 2;
pub const RLIMIT_FSIZE: c_int = 1;
pub const RLIMIT_MEMLOCK: c_int = 6;
pub const RLIMIT_NOFILE: c_int = 8;
pub const RLIMIT_NPROC: c_int = 7;
pub const RLIMIT_RSS: c_int = RLIMIT_AS;
pub const RLIMIT_STACK: c_int = 3;
pub const RLIM_INFINITY: rlim_t = 0x7fff_ffff_ffff_ffff;
pub const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
pub const RTLD_GLOBAL: c_int = 0x8;
pub const RTLD_LAZY: c_int = 0x1;
pub const RTLD_LOCAL: c_int = 0x4;
pub const RTLD_NOW: c_int = 0x2;
pub const SA_ONSTACK: c_int = 0x0001;
pub const SA_RESTART: c_int = 0x0002;
pub const SA_SIGINFO: c_int = 0x0040;
pub const SEEK_DATA: c_int = 4;
pub const SEEK_HOLE: c_int = 3;
pub const SF_APPEND: c_uint = 0x00040000;
pub const SF_ARCHIVED: c_uint = 0x00010000;
pub const SF_IMMUTABLE: c_uint = 0x00020000;
pub const SF_SETTABLE: c_uint = 0x3fff0000;
pub const SHUT_RD: c_int = 0;
pub const SHUT_RDWR: c_int = 2;
pub const SHUT_WR: c_int = 1;
pub const SIGBUS: c_int = 10;
pub const SIGCHLD: c_int = 20;
pub const SIGCONT: c_int = 19;
pub const SIGIO: c_int = 23;
pub const SIGSTOP: c_int = 17;
pub const SIGSYS: c_int = 12;
pub const SIGTSTP: c_int = 18;
pub const SIGURG: c_int = 16;
pub const SIGUSR1: c_int = 30;
pub const SIGUSR2: c_int = 31;
pub const SIG_BLOCK: c_int = 0x1;
pub const SIG_SETMASK: c_int = 3;
pub const SIG_UNBLOCK: c_int = 0x2;
pub const SOCK_DGRAM: c_int = 2;
pub const SOCK_STREAM: c_int = 1;
pub const SOL_SOCKET: c_int = 0xffff;
pub const SO_ERROR: c_int = 0x1007;
pub const SO_NOSIGPIPE: c_int = 0x1022;
pub const THOUSEP: nl_item = 51;
pub const TIOCGWINSZ: c_ulong = _IOR::<winsize>('t' as c_ulong, 104);
pub const TIOCPTYGNAME: c_uint = ulong_cast_uint(_IOC(IOC_OUT, 't' as c_ulong, 83, 128));
pub const TIOCSCTTY: c_uint = ulong_cast_uint(_IO('t' as c_ulong, 97));
pub const TIOCSWINSZ: c_ulong = _IOW::<winsize>('t' as c_ulong, 103);
pub const T_FMT: nl_item = 3;
pub const T_FMT_AMPM: nl_item = 4;
pub const UF_APPEND: c_uint = 0x00000004;
pub const UF_COMPRESSED: c_uint = 0x00000020;
pub const UF_HIDDEN: c_uint = 0x00008000;
pub const UF_IMMUTABLE: c_uint = 0x00000002;
pub const UF_NODUMP: c_uint = 0x00000001;
pub const UF_OPAQUE: c_uint = 0x00000008;
pub const UF_SETTABLE: c_uint = 0x0000ffff;
pub const UF_TRACKED: c_uint = 0x00000040;
pub const UTIME_OMIT: c_long = -2;
pub const VEOF: usize = 0;
pub const VMIN: usize = 16;
pub const VTIME: usize = 17;
pub const WCONTINUED: c_int = 0x00000010;
pub const WNOWAIT: c_int = 0x00000020;
pub const WSTOPPED: c_int = 0x00000008;
pub const YESEXPR: nl_item = 52;
pub const _CS_PATH: c_int = 1;
pub const _SC_CLK_TCK: c_int = 3;
pub const _SC_GETPW_R_SIZE_MAX: c_int = 71;
pub const _SC_PAGESIZE: c_int = 29;
pub const _SC_THREAD_STACK_MIN: c_int = 93;
pub const _WSTOPPED: c_int = 0o177;
pub const __PTHREAD_ATTR_SIZE__: usize = 56;
pub const kCCSuccess: i32 = 0;
#[inline]
pub unsafe extern "C" fn CMSG_DATA(cmsg: *const cmsghdr) -> *mut c_uchar {
    unsafe { (cmsg as *mut c_uchar).add(__DARWIN_ALIGN32(size_of::<cmsghdr>())) }
}
#[inline]
pub const unsafe extern "C" fn CMSG_LEN(length: c_uint) -> c_uint {
    unsafe { (__DARWIN_ALIGN32(size_of::<cmsghdr>()) + length as usize) as c_uint }
}
#[inline]
pub unsafe extern "C" fn CMSG_NXTHDR(mhdr: *const msghdr, cmsg: *const cmsghdr) -> *mut cmsghdr {
    unsafe {
        if cmsg.is_null() {
            return CMSG_FIRSTHDR(mhdr);
        }
        let cmsg_len = (*cmsg).cmsg_len as usize;
        let next = cmsg as usize + __DARWIN_ALIGN32(cmsg_len);
        let max = (*mhdr).msg_control as usize + (*mhdr).msg_controllen as usize;
        if next + __DARWIN_ALIGN32(size_of::<cmsghdr>()) > max {
            ptr::null_mut()
        } else {
            next as *mut cmsghdr
        }
    }
}
#[inline]
pub const unsafe extern "C" fn CMSG_SPACE(length: c_uint) -> c_uint {
    unsafe {
        (__DARWIN_ALIGN32(size_of::<cmsghdr>()) + __DARWIN_ALIGN32(length as usize)) as c_uint
    }
}
#[inline]
pub unsafe extern "C" fn FD_ISSET(fd: c_int, set: *const fd_set) -> bool {
    unsafe {
        let fd = fd as usize;
        let bits = size_of_val(&(*set).fds_bits[0]) * 8;
        let Some(slot) = (*set).fds_bits.get(fd / bits) else {
            {
                ::core::panic!(
                    "fd {0} out of range: valid fds are 0..FD_SETSIZE (0..{1})",
                    fd,
                    FD_SETSIZE
                );
            };
        };
        (*slot & (1 << (fd % bits))) != 0
    }
}
#[inline]
pub unsafe extern "C" fn FD_SET(fd: c_int, set: *mut fd_set) -> () {
    unsafe {
        let fd = fd as usize;
        let bits = size_of_val(&(*set).fds_bits[0]) * 8;
        let Some(slot) = (*set).fds_bits.get_mut(fd / bits) else {
            {
                ::core::panic!(
                    "fd {0} out of range: valid fds are 0..FD_SETSIZE (0..{1})",
                    fd,
                    FD_SETSIZE
                );
            };
        };
        *slot |= 1 << (fd % bits);
    }
}
#[inline]
pub const extern "C" fn WEXITSTATUS(status: c_int) -> c_int {
    (status >> 8) & 0x00ff
}
#[inline]
pub const extern "C" fn WIFEXITED(status: c_int) -> bool {
    (status & 0o177) == 0
}
#[inline]
pub const extern "C" fn WIFSIGNALED(status: c_int) -> bool {
    _WSTATUS(status) != _WSTOPPED && _WSTATUS(status) != 0
}
#[inline]
pub const extern "C" fn WTERMSIG(status: c_int) -> c_int {
    status & 0o177
}
pub const fn _IO(g: c_ulong, n: c_ulong) -> c_ulong {
    _IOC(IOC_VOID, g, n, 0)
}
pub(super) const fn _IOC(inout: u32, group: c_ulong, num: c_ulong, len: c_ulong) -> c_ulong {
    if cfg!(debug_assertions) {
        if !(inout <= IOC_DIRMASK) {
            ::core::panic!("assertion failed: inout <= IOC_DIRMASK")
        }
    }
    if cfg!(debug_assertions) {
        if !(group <= 0xff) {
            ::core::panic!("assertion failed: group <= 0xff")
        }
    }
    if cfg!(debug_assertions) {
        if !(num <= 0xff) {
            ::core::panic!("assertion failed: num <= 0xff")
        }
    }
    if cfg!(debug_assertions) {
        if !(len <= IOCPARM_MAX) {
            ::core::panic!("assertion failed: len <= IOCPARM_MAX")
        }
    }
    if !(size_of::<u32>() <= size_of::<c_ulong>()) {
        ::core::panic!("assertion failed: size_of::<u32>() <= size_of::<c_ulong>()")
    }
    (inout as c_ulong) | ((len & IOCPARM_MASK) << 16) | (group << 8) | num
}
pub const fn _IOR<T>(g: c_ulong, n: c_ulong) -> c_ulong {
    _IOC(IOC_OUT, g, n, mem::size_of::<T>() as c_ulong)
}
pub const fn _IOW<T>(g: c_ulong, n: c_ulong) -> c_ulong {
    _IOC(IOC_IN, g, n, mem::size_of::<T>() as c_ulong)
}
#[inline]
pub const extern "C" fn _WSTATUS(status: c_int) -> c_int {
    status & 0x7f
}
pub(super) const fn __DARWIN_ALIGN32(p: usize) -> usize {
    const __DARWIN_ALIGNBYTES32: usize = size_of::<u32>() - 1;
    (p + __DARWIN_ALIGNBYTES32) & !__DARWIN_ALIGNBYTES32
}
pub(super) const fn ulong_cast_uint(x: c_ulong) -> c_uint {
    assert!(x <= (c_uint::MAX as c_ulong));
    x as c_uint
}
unsafe extern "C" {
    pub fn CCRandomGenerateBytes(bytes: *mut c_void, size: size_t) -> CCRNGStatus;
    pub fn __error() -> *mut c_int;
    pub fn confstr(name: c_int, buf: *mut c_char, len: size_t) -> size_t;
    pub fn getentropy(buf: *mut c_void, buflen: size_t) -> c_int;
    pub fn getloadavg(loadavg: *mut c_double, nelem: c_int) -> c_int;
    pub fn getrlimit(resource: c_int, rlim: *mut rlimit) -> c_int;
    pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    pub fn kevent(
        kq: c_int,
        changelist: *const kevent,
        nchanges: c_int,
        eventlist: *mut kevent,
        nevents: c_int,
        timeout: *const timespec,
    ) -> c_int;
    pub fn kqueue() -> c_int;
    pub fn nl_langinfo(item: nl_item) -> *mut c_char;
    pub fn posix_spawn(
        pid: *mut pid_t,
        path: *const c_char,
        file_actions: *const posix_spawn_file_actions_t,
        attrp: *const posix_spawnattr_t,
        argv: *const *mut c_char,
        envp: *const *mut c_char,
    ) -> c_int;
    pub fn posix_spawn_file_actions_adddup2(
        actions: *mut posix_spawn_file_actions_t,
        fd: c_int,
        newfd: c_int,
    ) -> c_int;
    pub fn posix_spawn_file_actions_destroy(actions: *mut posix_spawn_file_actions_t) -> c_int;
    pub fn posix_spawn_file_actions_init(actions: *mut posix_spawn_file_actions_t) -> c_int;
    pub fn posix_spawnattr_destroy(attr: *mut posix_spawnattr_t) -> c_int;
    pub fn posix_spawnattr_init(attr: *mut posix_spawnattr_t) -> c_int;
    pub fn posix_spawnattr_setflags(attr: *mut posix_spawnattr_t, flags: c_short) -> c_int;
    pub fn posix_spawnattr_setpgroup(attr: *mut posix_spawnattr_t, flags: pid_t) -> c_int;
    pub fn posix_spawnattr_setsigdefault(
        attr: *mut posix_spawnattr_t,
        default: *const sigset_t,
    ) -> c_int;
    pub fn posix_spawnattr_setsigmask(
        attr: *mut posix_spawnattr_t,
        default: *const sigset_t,
    ) -> c_int;
    pub fn pthread_get_stackaddr_np(thread: pthread_t) -> *mut c_void;
    pub fn pthread_get_stacksize_np(thread: pthread_t) -> size_t;
    pub fn pthread_setname_np(name: *const c_char) -> c_int;
    pub fn setrlimit(resource: c_int, rlim: *const rlimit) -> c_int;
    pub fn strftime(
        buf: *mut c_char,
        maxsize: size_t,
        format: *const c_char,
        timeptr: *const tm,
    ) -> size_t;
    pub fn syscall(num: c_int, ...) -> c_int;
}
