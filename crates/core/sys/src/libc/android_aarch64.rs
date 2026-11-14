#[allow(unused_imports)]
use super::*;
pub type Ioctl = c_int;
pub type clock_t = c_long;
pub type clockid_t = c_int;
pub type dev_t = c_ulong;
pub type fsblkcnt_t = c_ulong;
pub type fsfilcnt_t = c_ulong;
pub type idtype_t = c_int;
pub type ino_t = c_ulong;
pub type mode_t = u32;
pub type nfds_t = c_uint;
pub type nlink_t = u32;
pub type off64_t = i64;
pub type off_t = c_long;
pub type posix_spawn_file_actions_t = *mut c_void;
pub type posix_spawnattr_t = *mut c_void;
pub type pthread_key_t = c_int;
pub type pthread_t = c_long;
pub type rlim_t = c_ulong;
pub type sa_family_t = u16;
pub type suseconds_t = c_long;
pub type tcflag_t = c_uint;
pub type time_t = c_long;
pub type wchar_t = u32;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct cmsghdr {
    pub cmsg_len: size_t,
    pub cmsg_level: c_int,
    pub cmsg_type: c_int,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct epoll_event {
    pub events: u32,
    pub u64: u64,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct fd_set {
    pub(super) fds_bits: [c_ulong; FD_SETSIZE as usize / ULONG_SIZE],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct in_addr {
    pub s_addr: in_addr_t,
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
    pub int_p_sep_by_space: c_char,
    pub int_n_cs_precedes: c_char,
    pub int_n_sep_by_space: c_char,
    pub int_p_sign_posn: c_char,
    pub int_n_sign_posn: c_char,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct msghdr {
    pub msg_name: *mut c_void,
    pub msg_namelen: socklen_t,
    pub msg_iov: *mut iovec,
    pub msg_iovlen: size_t,
    pub msg_control: *mut c_void,
    pub msg_controllen: size_t,
    pub msg_flags: c_int,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct passwd {
    pub pw_name: *mut c_char,
    pub pw_passwd: *mut c_char,
    pub pw_uid: uid_t,
    pub pw_gid: gid_t,
    pub pw_gecos: *mut c_char,
    pub pw_dir: *mut c_char,
    pub pw_shell: *mut c_char,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct pthread_attr_t {
    pub flags: u32,
    pub stack_base: *mut c_void,
    pub stack_size: size_t,
    pub guard_size: size_t,
    pub sched_policy: i32,
    pub sched_priority: i32,
    pub(super) __reserved: Padding<[c_char; 16]>,
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
}
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) union sifields {
    pub(super) _align_pointer: *mut c_void,
    pub(super) sigchld: sifields_sigchld,
}
impl ::core::fmt::Debug for sifields {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("sifields").finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct sifields_sigchld {
    pub(super) si_pid: pid_t,
    pub(super) si_uid: uid_t,
    pub(super) si_status: c_int,
    pub(super) si_utime: c_long,
    pub(super) si_stime: c_long,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
#[allow(unpredictable_function_pointer_comparisons)]
pub struct sigaction {
    pub sa_flags: c_int,
    pub sa_sigaction: sighandler_t,
    pub sa_mask: sigset_t,
    pub sa_restorer: Option<extern "C" fn()>,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct siginfo_f {
    pub(super) _siginfo_base: [c_int; 3],
    pub(super) sifields: sifields,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct siginfo_t {
    pub si_signo: c_int,
    pub si_errno: c_int,
    pub si_code: c_int,
    pub _pad: [c_int; 29],
    pub(super) _align: [usize; 0],
}
impl siginfo_t {
    pub unsafe fn si_addr(&self) -> *mut c_void {
        unsafe {
            #[repr(C)]
            struct siginfo_sigfault {
                pub(super) _si_signo: c_int,
                pub(super) _si_errno: c_int,
                pub(super) _si_code: c_int,
                pub(super) si_addr: *mut c_void,
            }
            (*(self as *const siginfo_t).cast::<siginfo_sigfault>()).si_addr
        }
    }
    pub unsafe fn si_value(&self) -> sigval {
        unsafe {
            #[repr(C)]
            struct siginfo_timer {
                pub(super) _si_signo: c_int,
                pub(super) _si_errno: c_int,
                pub(super) _si_code: c_int,
                pub(super) _si_tid: c_int,
                pub(super) _si_overrun: c_int,
                pub(super) si_sigval: sigval,
            }
            (*(self as *const siginfo_t).cast::<siginfo_timer>()).si_sigval
        }
    }
}
impl siginfo_t {
    unsafe fn sifields(&self) -> &sifields {
        unsafe { &(*(self as *const siginfo_t).cast::<siginfo_f>()).sifields }
    }
    pub unsafe fn si_pid(&self) -> pid_t {
        unsafe { self.sifields().sigchld.si_pid }
    }
    pub unsafe fn si_uid(&self) -> uid_t {
        unsafe { self.sifields().sigchld.si_uid }
    }
    pub unsafe fn si_status(&self) -> c_int {
        unsafe { self.sifields().sigchld.si_status }
    }
    pub unsafe fn si_utime(&self) -> c_long {
        unsafe { self.sifields().sigchld.si_utime }
    }
    pub unsafe fn si_stime(&self) -> c_long {
        unsafe { self.sifields().sigchld.si_stime }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sigset_t {
    pub(super) __val: [c_ulong; 1],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr {
    pub sa_family: sa_family_t,
    pub sa_data: [c_char; 14],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr_alg {
    pub salg_family: sa_family_t,
    pub salg_type: [c_uchar; 14],
    pub salg_feat: u32,
    pub salg_mask: u32,
    pub salg_name: [c_uchar; 64],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr_in {
    pub sin_family: sa_family_t,
    pub sin_port: in_port_t,
    pub sin_addr: in_addr,
    pub sin_zero: [u8; 8],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr_in6 {
    pub sin6_family: sa_family_t,
    pub sin6_port: in_port_t,
    pub sin6_flowinfo: u32,
    pub sin6_addr: in6_addr,
    pub sin6_scope_id: u32,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr_storage {
    pub ss_family: sa_family_t,
    pub(super) __ss_pad2: Padding<[u8; 128 - 2 - 8]>,
    pub(super) __ss_align: size_t,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct stat {
    pub st_dev: dev_t,
    pub st_ino: ino_t,
    pub st_mode: c_uint,
    pub st_nlink: nlink_t,
    pub st_uid: uid_t,
    pub st_gid: gid_t,
    pub st_rdev: dev_t,
    pub(super) __pad1: Padding<c_ulong>,
    pub st_size: off64_t,
    pub st_blksize: c_int,
    pub(super) __pad2: Padding<c_int>,
    pub st_blocks: c_long,
    pub st_atime: time_t,
    pub st_atime_nsec: c_long,
    pub st_mtime: time_t,
    pub st_mtime_nsec: c_long,
    pub st_ctime: time_t,
    pub st_ctime_nsec: c_long,
    pub(super) __unused4: Padding<c_uint>,
    pub(super) __unused5: Padding<c_uint>,
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
    pub(super) __f_reserved: Padding<[u32; 6]>,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct termios {
    pub c_iflag: tcflag_t,
    pub c_oflag: tcflag_t,
    pub c_cflag: tcflag_t,
    pub c_lflag: tcflag_t,
    pub c_line: cc_t,
    pub c_cc: [cc_t; NCCS],
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
    pub tm_zone: *const c_char,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct utsname {
    pub sysname: [c_char; 65],
    pub nodename: [c_char; 65],
    pub release: [c_char; 65],
    pub version: [c_char; 65],
    pub machine: [c_char; 65],
    pub domainname: [c_char; 65],
}
pub const AF_ALG: c_int = 38;
pub const AF_BLUETOOTH: c_int = 31;
pub const AF_CAN: c_int = 29;
pub const AF_INET6: c_int = 10;
pub const AF_UNIX: c_int = 1;
pub const AI_NUMERICHOST: c_int = 0x00000004;
pub const AI_PASSIVE: c_int = 0x00000001;
pub const AT_FDCWD: c_int = -100;
pub const AT_REMOVEDIR: c_int = 0x200;
pub const AT_SYMLINK_FOLLOW: c_int = 0x400;
pub const AT_SYMLINK_NOFOLLOW: c_int = 0x100;
pub const CLOCK_BOOTTIME: clockid_t = 7;
pub const CLOCK_MONOTONIC: clockid_t = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: clockid_t = 2;
pub const CLOCK_TAI: clockid_t = 11;
pub const CLOCK_THREAD_CPUTIME_ID: clockid_t = 3;
pub const CS8: tcflag_t = 0x00000030;
pub const CSIZE: tcflag_t = 0x00000030;
pub const EADDRINUSE: c_int = 98;
pub const EADDRNOTAVAIL: c_int = 99;
pub const EADV: c_int = 68;
pub const EAFNOSUPPORT: c_int = 97;
pub const EAGAIN: c_int = 11;
pub const EALREADY: c_int = 114;
pub const EBADE: c_int = 52;
pub const EBADFD: c_int = 77;
pub const EBADMSG: c_int = 74;
pub const EBADR: c_int = 53;
pub const EBADRQC: c_int = 56;
pub const EBADSLT: c_int = 57;
pub const EBFONT: c_int = 59;
pub const ECANCELED: c_int = 125;
pub const ECHONL: tcflag_t = 0x00000040;
pub const ECHRNG: c_int = 44;
pub const ECOMM: c_int = 70;
pub const ECONNABORTED: c_int = 103;
pub const ECONNREFUSED: c_int = 111;
pub const ECONNRESET: c_int = 104;
pub const EDEADLK: c_int = 35;
pub const EDESTADDRREQ: c_int = 89;
pub const EDOTDOT: c_int = 73;
pub const EDQUOT: c_int = 122;
pub const EFD_CLOEXEC: c_int = O_CLOEXEC;
pub const EFD_NONBLOCK: c_int = O_NONBLOCK;
pub const EHOSTDOWN: c_int = 112;
pub const EHOSTUNREACH: c_int = 113;
pub const EIDRM: c_int = 43;
pub const EILSEQ: c_int = 84;
pub const EINPROGRESS: c_int = 115;
pub const EISCONN: c_int = 106;
pub const EISNAM: c_int = 120;
pub const EKEYEXPIRED: c_int = 127;
pub const EKEYREJECTED: c_int = 129;
pub const EKEYREVOKED: c_int = 128;
pub const EL2HLT: c_int = 51;
pub const EL2NSYNC: c_int = 45;
pub const EL3HLT: c_int = 46;
pub const EL3RST: c_int = 47;
pub const ELIBACC: c_int = 79;
pub const ELIBBAD: c_int = 80;
pub const ELIBEXEC: c_int = 83;
pub const ELIBMAX: c_int = 82;
pub const ELIBSCN: c_int = 81;
pub const ELNRNG: c_int = 48;
pub const ELOOP: c_int = 40;
pub const EMEDIUMTYPE: c_int = 124;
pub const EMSGSIZE: c_int = 90;
pub const EMULTIHOP: c_int = 72;
pub const ENAMETOOLONG: c_int = 36;
pub const ENAVAIL: c_int = 119;
pub const ENETDOWN: c_int = 100;
pub const ENETRESET: c_int = 102;
pub const ENETUNREACH: c_int = 101;
pub const ENOANO: c_int = 55;
pub const ENOBUFS: c_int = 105;
pub const ENOCSI: c_int = 50;
pub const ENODATA: c_int = 61;
pub const ENOKEY: c_int = 126;
pub const ENOLCK: c_int = 37;
pub const ENOLINK: c_int = 67;
pub const ENOMEDIUM: c_int = 123;
pub const ENOMSG: c_int = 42;
pub const ENONET: c_int = 64;
pub const ENOPKG: c_int = 65;
pub const ENOPROTOOPT: c_int = 92;
pub const ENOSR: c_int = 63;
pub const ENOSTR: c_int = 60;
pub const ENOSYS: c_int = 38;
pub const ENOTCONN: c_int = 107;
pub const ENOTEMPTY: c_int = 39;
pub const ENOTNAM: c_int = 118;
pub const ENOTRECOVERABLE: c_int = 131;
pub const ENOTSOCK: c_int = 88;
pub const ENOTSUP: c_int = EOPNOTSUPP;
pub const ENOTUNIQ: c_int = 76;
pub const EOPNOTSUPP: c_int = 95;
pub const EOVERFLOW: c_int = 75;
pub const EOWNERDEAD: c_int = 130;
pub const EPFNOSUPPORT: c_int = 96;
pub const EPOLLERR: c_int = 0x8;
pub const EPOLLET: c_int = u32_cast_int(0x80000000);
pub const EPOLLHUP: c_int = 0x10;
pub const EPOLLIN: c_int = 0x1;
pub const EPOLLOUT: c_int = 0x4;
pub const EPOLLPRI: c_int = 0x2;
pub const EPOLLRDHUP: c_int = 0x2000;
pub const EPOLL_CLOEXEC: c_int = 0x80000;
pub const EPOLL_CTL_ADD: c_int = 1;
pub const EPOLL_CTL_DEL: c_int = 2;
pub const EPROTO: c_int = 71;
pub const EPROTONOSUPPORT: c_int = 93;
pub const EPROTOTYPE: c_int = 91;
pub const EREMCHG: c_int = 78;
pub const EREMOTE: c_int = 66;
pub const EREMOTEIO: c_int = 121;
pub const ERESTART: c_int = 85;
pub const ESHUTDOWN: c_int = 108;
pub const ESOCKTNOSUPPORT: c_int = 94;
pub const ESRMNT: c_int = 69;
pub const ESTALE: c_int = 116;
pub const ESTRPIPE: c_int = 86;
pub const ETIME: c_int = 62;
pub const ETIMEDOUT: c_int = 110;
pub const ETOOMANYREFS: c_int = 109;
pub const EUCLEAN: c_int = 117;
pub const EUNATCH: c_int = 49;
pub const EUSERS: c_int = 87;
pub const EXFULL: c_int = 54;
pub const FIONBIO: c_int = 0x5421;
pub const F_DUPFD_CLOEXEC: c_int = 1030;
pub const ICANON: tcflag_t = 0x00000002;
pub const IEXTEN: tcflag_t = 0x00008000;
pub const ISIG: tcflag_t = 0x00000001;
pub const IXON: tcflag_t = 0x00000400;
pub const LC_ALL: c_int = 6;
pub const LC_COLLATE: c_int = 3;
pub const LC_CTYPE: c_int = 0;
pub const LC_MESSAGES: c_int = 5;
pub const LC_MONETARY: c_int = 4;
pub const LC_NUMERIC: c_int = 1;
pub const LC_TIME: c_int = 2;
pub const MAP_ANONYMOUS: c_int = 0x0020;
pub const MSG_CMSG_CLOEXEC: c_int = 0x40000000;
pub const MSG_FASTOPEN: c_int = 0x20000000;
pub const NCCS: usize = 19;
pub const O_ACCMODE: c_int = 3;
pub const O_APPEND: c_int = 1024;
pub const O_ASYNC: c_int = 0x2000;
pub const O_CLOEXEC: c_int = 0x80000;
pub const O_CREAT: c_int = 64;
pub const O_DIRECT: c_int = 0x10000;
pub const O_DIRECTORY: c_int = 0x4000;
pub const O_DSYNC: c_int = 4096;
pub const O_EXCL: c_int = 128;
pub const O_LARGEFILE: c_int = 0o400000;
pub const O_NDELAY: c_int = 0x800;
pub const O_NOATIME: c_int = 0o1000000;
pub const O_NOCTTY: c_int = 256;
pub const O_NOFOLLOW: c_int = 0x8000;
pub const O_NONBLOCK: c_int = 2048;
pub const O_PATH: c_int = 0o10000000;
pub const O_RSYNC: c_int = O_SYNC;
pub const O_SYNC: c_int = 0x101000;
pub const O_TMPFILE: c_int = 0o20000000 | O_DIRECTORY;
pub const O_TRUNC: c_int = 512;
pub const PARENB: tcflag_t = 0x00000100;
pub const PIPE_BUF: usize = 4096;
pub const POSIX_SPAWN_CLOEXEC_DEFAULT: c_short = 256;
pub const POSIX_SPAWN_SETPGROUP: c_short = 2;
pub const POSIX_SPAWN_SETSID: c_short = 128;
pub const POSIX_SPAWN_SETSIGDEF: c_short = 4;
pub const POSIX_SPAWN_SETSIGMASK: c_short = 8;
pub const PR_SET_DUMPABLE: c_int = 4;
pub const PR_SET_NAME: c_int = 15;
pub const PR_SET_NO_NEW_PRIVS: c_int = 38;
pub const PTHREAD_CREATE_DETACHED: c_int = 1;
pub const RLIMIT_AS: c_int = 9;
pub const RLIMIT_CORE: c_int = 4;
pub const RLIMIT_CPU: c_int = 0;
pub const RLIMIT_DATA: c_int = 2;
pub const RLIMIT_FSIZE: c_int = 1;
pub const RLIMIT_LOCKS: c_int = 10;
pub const RLIMIT_MEMLOCK: c_int = 8;
pub const RLIMIT_MSGQUEUE: c_int = 12;
pub const RLIMIT_NICE: c_int = 13;
pub const RLIMIT_NOFILE: c_int = 7;
pub const RLIMIT_NPROC: c_int = 6;
pub const RLIMIT_RSS: c_int = 5;
pub const RLIMIT_RTPRIO: c_int = 14;
pub const RLIMIT_SIGPENDING: c_int = 11;
pub const RLIMIT_STACK: c_int = 3;
pub const RLIM_INFINITY: rlim_t = !0;
pub const RTLD_DEFAULT: *mut c_void = ptr::null_mut();
pub const RTLD_GLOBAL: c_int = 0x00100;
pub const RTLD_LAZY: c_int = 1;
pub const RTLD_LOCAL: c_int = 0;
pub const RTLD_NOW: c_int = 2;
pub const SA_ONSTACK: c_int = 0x08000000;
pub const SA_RESTART: c_int = 0x10000000;
pub const SA_SIGINFO: c_int = 0x00000004;
pub const SECCOMP_SET_MODE_FILTER: c_uint = 1;
pub const SEEK_DATA: c_int = 3;
pub const SEEK_HOLE: c_int = 4;
pub const SHUT_RD: c_int = 0;
pub const SHUT_RDWR: c_int = 2;
pub const SHUT_WR: c_int = 1;
pub const SIGBUS: c_int = 7;
pub const SIGCHLD: c_int = 17;
pub const SIGCONT: c_int = 18;
pub const SIGIO: c_int = 29;
pub const SIGPWR: c_int = 30;
pub const SIGSTKFLT: c_int = 16;
pub const SIGSTOP: c_int = 19;
pub const SIGSYS: c_int = 31;
pub const SIGTSTP: c_int = 20;
pub const SIGURG: c_int = 23;
pub const SIGUSR1: c_int = 10;
pub const SIGUSR2: c_int = 12;
pub const SIG_BLOCK: c_int = 0x000000;
pub const SIG_SETMASK: c_int = 2;
pub const SIG_UNBLOCK: c_int = 0x01;
pub const SOCK_CLOEXEC: c_int = O_CLOEXEC;
pub const SOCK_DGRAM: c_int = 2;
pub const SOCK_NONBLOCK: c_int = O_NONBLOCK;
pub const SOCK_STREAM: c_int = 1;
pub const SOL_SOCKET: c_int = 1;
pub const SO_ERROR: c_int = 4;
pub const SYS_accept: c_long = 202;
pub const SYS_accept4: c_long = 242;
pub const SYS_close_range: c_long = 436;
pub const SYS_io_uring_enter: c_long = 426;
pub const SYS_io_uring_register: c_long = 427;
pub const SYS_io_uring_setup: c_long = 425;
pub const SYS_landlock_add_rule: c_long = 445;
pub const SYS_landlock_create_ruleset: c_long = 444;
pub const SYS_landlock_restrict_self: c_long = 446;
pub const SYS_listen: c_long = 201;
pub const SYS_pidfd_open: c_long = 434;
pub const SYS_pidfd_send_signal: c_long = 424;
pub const SYS_seccomp: c_long = 277;
pub const SYS_sendmmsg: c_long = 269;
pub const SYS_sendmsg: c_long = 211;
pub const SYS_sendto: c_long = 206;
pub const SYS_setpgid: c_long = 154;
pub const SYS_setsid: c_long = 157;
pub const SYS_socket: c_long = 198;
pub const TIOCGWINSZ: c_int = 0x5413;
pub const TIOCSCTTY: c_int = 0x540E;
pub const TIOCSWINSZ: c_int = 0x5414;
pub(super) const ULONG_SIZE: usize = 64;
pub const UTIME_OMIT: c_long = 1073741822;
pub const VEOF: usize = 4;
pub const VMIN: usize = 6;
pub const VTIME: usize = 5;
pub const WCONTINUED: c_int = 0x00000008;
pub const WNOWAIT: c_int = 0x01000000;
pub const WSTOPPED: c_int = WUNTRACED;
pub const WUNTRACED: c_int = 0x00000002;
pub(super) const _IOC_DIRBITS: u32 = 2;
pub(super) const _IOC_DIRMASK: u32 = (1 << _IOC_DIRBITS) - 1;
pub(super) const _IOC_DIRSHIFT: u32 = _IOC_SIZESHIFT + _IOC_SIZEBITS;
pub(super) const _IOC_NONE: u32 = 0;
pub(super) const _IOC_NRBITS: u32 = 8;
pub(super) const _IOC_NRMASK: u32 = (1 << _IOC_NRBITS) - 1;
pub(super) const _IOC_NRSHIFT: u32 = 0;
pub(super) const _IOC_SIZEBITS: u32 = 14;
pub(super) const _IOC_SIZEMASK: u32 = (1 << _IOC_SIZEBITS) - 1;
pub(super) const _IOC_SIZESHIFT: u32 = _IOC_TYPESHIFT + _IOC_TYPEBITS;
pub(super) const _IOC_TYPEBITS: u32 = 8;
pub(super) const _IOC_TYPEMASK: u32 = (1 << _IOC_TYPEBITS) - 1;
pub(super) const _IOC_TYPESHIFT: u32 = _IOC_NRSHIFT + _IOC_NRBITS;
pub const _SC_CLK_TCK: c_int = 0x0006;
pub const _SC_GETPW_R_SIZE_MAX: c_int = 0x0048;
pub const _SC_PAGESIZE: c_int = 0x0027;
pub const _SC_THREAD_STACK_MIN: c_int = 0x004c;
pub(super) const fn CMSG_ALIGN(len: usize) -> usize {
    (len + size_of::<usize>() - 1) & !(size_of::<usize>() - 1)
}
#[inline]
pub unsafe extern "C" fn CMSG_DATA(cmsg: *const cmsghdr) -> *mut c_uchar {
    unsafe { cmsg.offset(1) as *mut c_uchar }
}
#[inline]
pub const unsafe extern "C" fn CMSG_LEN(length: c_uint) -> c_uint {
    unsafe { CMSG_ALIGN(size_of::<cmsghdr>()) as c_uint + length }
}
#[inline]
pub unsafe extern "C" fn CMSG_NXTHDR(mhdr: *const msghdr, cmsg: *const cmsghdr) -> *mut cmsghdr {
    unsafe {
        let next = (cmsg as usize + CMSG_ALIGN((*cmsg).cmsg_len as usize)) as *mut cmsghdr;
        let max = (*mhdr).msg_control as usize + (*mhdr).msg_controllen as usize;
        if (next.offset(1)) as usize > max {
            ptr::null_mut()
        } else {
            next.cast()
        }
    }
}
#[inline]
pub const unsafe extern "C" fn CMSG_SPACE(length: c_uint) -> c_uint {
    unsafe { (CMSG_ALIGN(length as usize) + CMSG_ALIGN(size_of::<cmsghdr>())) as c_uint }
}
#[inline]
pub unsafe extern "C" fn FD_ISSET(fd: c_int, set: *const fd_set) -> bool {
    unsafe {
        let fd = fd as usize;
        let size = size_of_val(&(*set).fds_bits[0]) * 8;
        let Some(slot) = (*set).fds_bits.get(fd / size) else {
            {
                ::core::panic!(
                    "fd {0} out of range: valid fds are 0..FD_SETSIZE (0..{1})",
                    fd,
                    FD_SETSIZE
                );
            };
        };
        (*slot & (1 << (fd % size))) != 0
    }
}
#[inline]
pub unsafe extern "C" fn FD_SET(fd: c_int, set: *mut fd_set) -> () {
    unsafe {
        let fd = fd as usize;
        let size = size_of_val(&(*set).fds_bits[0]) * 8;
        let Some(slot) = (*set).fds_bits.get_mut(fd / size) else {
            {
                ::core::panic!(
                    "fd {0} out of range: valid fds are 0..FD_SETSIZE (0..{1})",
                    fd,
                    FD_SETSIZE
                );
            };
        };
        *slot |= 1 << (fd % size);
    }
}
#[inline]
pub const extern "C" fn WEXITSTATUS(status: c_int) -> c_int {
    (status >> 8) & 0xff
}
#[inline]
pub const extern "C" fn WIFEXITED(status: c_int) -> bool {
    (status & 0x7f) == 0
}
#[inline]
pub const extern "C" fn WIFSIGNALED(status: c_int) -> bool {
    ((status & 0x7f) + 1) as i8 >= 2
}
#[inline]
pub const extern "C" fn WTERMSIG(status: c_int) -> c_int {
    status & 0x7f
}
pub const fn _IO(ty: u32, nr: u32) -> Ioctl {
    _IOC(_IOC_NONE, ty, nr, 0)
}
pub(super) const fn _IOC(dir: u32, ty: u32, nr: u32, size: usize) -> Ioctl {
    if cfg!(debug_assertions) {
        if !(dir <= _IOC_DIRMASK) {
            ::core::panic!("assertion failed: dir <= _IOC_DIRMASK")
        }
    }
    if cfg!(debug_assertions) {
        if !(ty <= _IOC_TYPEMASK) {
            ::core::panic!("assertion failed: ty <= _IOC_TYPEMASK")
        }
    }
    if cfg!(debug_assertions) {
        if !(nr <= _IOC_NRMASK) {
            ::core::panic!("assertion failed: nr <= _IOC_NRMASK")
        }
    }
    if cfg!(debug_assertions) {
        if !(size <= (_IOC_SIZEMASK as usize)) {
            ::core::panic!("assertion failed: size <= (_IOC_SIZEMASK as usize)")
        }
    }
    ((dir << _IOC_DIRSHIFT)
        | (ty << _IOC_TYPESHIFT)
        | (nr << _IOC_NRSHIFT)
        | ((size as u32) << _IOC_SIZESHIFT)) as Ioctl
}
pub(super) const fn u32_cast_int(x: u32) -> c_int {
    x as i32
}
unsafe extern "C" {
    pub fn __errno() -> *mut c_int;
    pub fn clock_settime(clk_id: clockid_t, tp: *const timespec) -> c_int;
    pub fn clone(
        cb: extern "C" fn(*mut c_void) -> c_int,
        child_stack: *mut c_void,
        flags: c_int,
        arg: *mut c_void,
        ...
    ) -> c_int;
    pub fn close(fd: c_int) -> c_int;
    pub fn epoll_create1(flags: c_int) -> c_int;
    pub fn epoll_ctl(epfd: c_int, op: c_int, fd: c_int, event: *mut epoll_event) -> c_int;
    pub fn epoll_wait(
        epfd: c_int,
        events: *mut epoll_event,
        maxevents: c_int,
        timeout: c_int,
    ) -> c_int;
    pub fn eventfd(initval: c_uint, flags: c_int) -> c_int;
    pub fn fstat(fildes: c_int, buf: *mut stat) -> c_int;
    pub fn getentropy(buf: *mut c_void, buflen: size_t) -> c_int;
    pub fn getrandom(buf: *mut c_void, buflen: size_t, flags: c_uint) -> ssize_t;
    pub fn getrlimit(resource: c_int, rlim: *mut rlimit) -> c_int;
    pub fn ioctl(fd: c_int, request: Ioctl, ...) -> c_int;
    pub fn prctl(option: c_int, ...) -> c_int;
    pub fn pthread_attr_getstack(
        attr: *const pthread_attr_t,
        stackaddr: *mut *mut c_void,
        stacksize: *mut size_t,
    ) -> c_int;
    pub fn pthread_getattr_np(native: pthread_t, attr: *mut pthread_attr_t) -> c_int;
    pub fn pthread_setname_np(thread: pthread_t, name: *const c_char) -> c_int;
    pub fn select(
        nfds: c_int,
        readfds: *mut fd_set,
        writefds: *mut fd_set,
        errorfds: *mut fd_set,
        timeout: *mut timeval,
    ) -> c_int;
    pub fn setrlimit(resource: c_int, rlim: *const rlimit) -> c_int;
    pub fn stat(path: *const c_char, buf: *mut stat) -> c_int;
    pub fn strftime(s: *mut c_char, max: size_t, format: *const c_char, tm: *const tm) -> size_t;
    pub fn syscall(num: c_long, ...) -> c_long;
    pub fn system(s: *const c_char) -> c_int;
}
