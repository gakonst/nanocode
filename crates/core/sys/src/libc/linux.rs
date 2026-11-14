#[allow(unused_imports)]
use super::*;
pub use core::ffi::c_double;
pub type blkcnt_t = i64;
pub type canid_t = u32;
pub type clockid_t = c_int;
pub type dev_t = u64;
pub type idtype_t = c_uint;
pub type ino_t = u64;
pub type mode_t = u32;
pub type nfds_t = c_ulong;
pub type nl_item = c_int;
pub type off_t = i64;
pub type pthread_key_t = c_uint;
pub type sa_family_t = u16;
pub type speed_t = c_uint;
pub type tcflag_t = c_uint;
#[derive(Clone, Copy)]
#[repr(C)]
pub union __c_anonymous_sockaddr_can_can_addr {
    pub tp: __c_anonymous_sockaddr_can_tp,
    pub j1939: __c_anonymous_sockaddr_can_j1939,
}
impl ::core::fmt::Debug for __c_anonymous_sockaddr_can_can_addr {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("__c_anonymous_sockaddr_can_can_addr")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct __c_anonymous_sockaddr_can_j1939 {
    pub name: u64,
    pub pgn: u32,
    pub addr: u8,
}
impl ::core::default::Default for __c_anonymous_sockaddr_can_j1939 {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            name: ::core::default::Default::default(),
            pgn: ::core::default::Default::default(),
            addr: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct __c_anonymous_sockaddr_can_tp {
    pub rx_id: canid_t,
    pub tx_id: canid_t,
}
impl ::core::default::Default for __c_anonymous_sockaddr_can_tp {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            rx_id: ::core::default::Default::default(),
            tx_id: ::core::default::Default::default(),
        }
    }
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
pub struct posix_spawn_file_actions_t {
    pub(super) __allocated: c_int,
    pub(super) __used: c_int,
    pub(super) __actions: *mut c_int,
    pub(super) __pad: Padding<[c_int; 16]>,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
#[allow(unpredictable_function_pointer_comparisons)]
pub struct sigaction {
    pub sa_sigaction: sighandler_t,
    pub sa_mask: sigset_t,
    pub sa_flags: c_int,
    pub sa_restorer: Option<extern "C" fn()>,
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
pub struct sockaddr_can {
    pub can_family: sa_family_t,
    pub can_ifindex: c_int,
    pub can_addr: __c_anonymous_sockaddr_can_can_addr,
}
impl ::core::default::Default for sockaddr_can {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            can_family: ::core::default::Default::default(),
            can_ifindex: ::core::default::Default::default(),
            can_addr: unsafe { mem::zeroed::<__c_anonymous_sockaddr_can_can_addr>() },
        }
    }
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
pub const ABDAY_1: nl_item = DATE_BASE;
pub const ABDAY_2: nl_item = DATE_BASE + 0x1;
pub const ABDAY_3: nl_item = DATE_BASE + 0x2;
pub const ABDAY_4: nl_item = DATE_BASE + 0x3;
pub const ABDAY_5: nl_item = DATE_BASE + 0x4;
pub const ABDAY_6: nl_item = DATE_BASE + 0x5;
pub const ABDAY_7: nl_item = DATE_BASE + 0x6;
pub const ABMON_1: nl_item = DATE_BASE + 0xE;
pub const ABMON_10: nl_item = DATE_BASE + 0x17;
pub const ABMON_11: nl_item = DATE_BASE + 0x18;
pub const ABMON_12: nl_item = DATE_BASE + 0x19;
pub const ABMON_2: nl_item = DATE_BASE + 0xF;
pub const ABMON_3: nl_item = DATE_BASE + 0x10;
pub const ABMON_4: nl_item = DATE_BASE + 0x11;
pub const ABMON_5: nl_item = DATE_BASE + 0x12;
pub const ABMON_6: nl_item = DATE_BASE + 0x13;
pub const ABMON_7: nl_item = DATE_BASE + 0x14;
pub const ABMON_8: nl_item = DATE_BASE + 0x15;
pub const ABMON_9: nl_item = DATE_BASE + 0x16;
pub const AF_ALG: c_int = 38;
pub const AF_BLUETOOTH: c_int = 31;
pub const AF_CAN: c_int = 29;
pub const AF_INET6: c_int = 10;
pub const AF_UNIX: c_int = 1;
pub const AI_NUMERICHOST: c_int = 0x0004;
pub const AI_PASSIVE: c_int = 0x0001;
pub const ALT_DIGITS: nl_item = DATE_BASE + 0x2F;
pub const AM_STR: nl_item = DATE_BASE + 0x26;
pub const AT_EACCESS: c_int = 0x200;
pub const AT_FDCWD: c_int = -100;
pub const AT_REMOVEDIR: c_int = 0x200;
pub const AT_SYMLINK_FOLLOW: c_int = 0x400;
pub const AT_SYMLINK_NOFOLLOW: c_int = 0x100;
pub const CAN_BCM: c_int = 2;
pub const CAN_ISOTP: c_int = 6;
pub const CAN_J1939: c_int = 7;
pub const CAN_RAW: c_int = 1;
pub const CLOCK_BOOTTIME: clockid_t = 7;
pub const CLOCK_MONOTONIC: clockid_t = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: clockid_t = 2;
pub const CLOCK_TAI: clockid_t = 11;
pub const CLOCK_THREAD_CPUTIME_ID: clockid_t = 3;
pub const CODESET: nl_item = 14;
pub const CRNCYSTR: nl_item = 0x4000F;
pub const CS8: tcflag_t = 0x00000030;
pub const CSIZE: tcflag_t = 0x00000030;
pub(super) const DATE_BASE: nl_item = 0x20000;
pub const DAY_1: nl_item = DATE_BASE + 0x7;
pub const DAY_2: nl_item = DATE_BASE + 0x8;
pub const DAY_3: nl_item = DATE_BASE + 0x9;
pub const DAY_4: nl_item = DATE_BASE + 0xA;
pub const DAY_5: nl_item = DATE_BASE + 0xB;
pub const DAY_6: nl_item = DATE_BASE + 0xC;
pub const DAY_7: nl_item = DATE_BASE + 0xD;
pub const D_FMT: nl_item = DATE_BASE + 0x29;
pub const D_T_FMT: nl_item = DATE_BASE + 0x28;
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
pub const EFD_CLOEXEC: c_int = 0x80000;
pub const EHOSTDOWN: c_int = 112;
pub const EHOSTUNREACH: c_int = 113;
pub const EHWPOISON: c_int = 133;
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
pub const ERA: nl_item = DATE_BASE + 0x2C;
pub const ERA_D_FMT: nl_item = DATE_BASE + 0x2E;
pub const ERA_D_T_FMT: nl_item = DATE_BASE + 0x30;
pub const ERA_T_FMT: nl_item = DATE_BASE + 0x31;
pub const EREMCHG: c_int = 78;
pub const EREMOTE: c_int = 66;
pub const EREMOTEIO: c_int = 121;
pub const ERESTART: c_int = 85;
pub const ERFKILL: c_int = 132;
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
pub const FIONBIO: Ioctl = 0x5421;
pub const F_DUPFD_CLOEXEC: c_int = 1030;
pub const ICANON: tcflag_t = 0x00000002;
pub const IEXTEN: tcflag_t = 0x00008000;
pub const ISIG: tcflag_t = 0x00000001;
pub const ITIMER_PROF: c_int = 2;
pub const ITIMER_REAL: c_int = 0;
pub const ITIMER_VIRTUAL: c_int = 1;
pub const IXON: tcflag_t = 0x00000400;
pub const LC_ALL: c_int = 6;
pub const LC_COLLATE: c_int = 3;
pub const LC_CTYPE: c_int = 0;
pub const LC_MESSAGES: c_int = 5;
pub const LC_MONETARY: c_int = 4;
pub const LC_NUMERIC: c_int = 1;
pub const LC_TIME: c_int = 2;
pub const MON_1: nl_item = DATE_BASE + 0x1A;
pub const MON_10: nl_item = DATE_BASE + 0x23;
pub const MON_11: nl_item = DATE_BASE + 0x24;
pub const MON_12: nl_item = DATE_BASE + 0x25;
pub const MON_2: nl_item = DATE_BASE + 0x1B;
pub const MON_3: nl_item = DATE_BASE + 0x1C;
pub const MON_4: nl_item = DATE_BASE + 0x1D;
pub const MON_5: nl_item = DATE_BASE + 0x1E;
pub const MON_6: nl_item = DATE_BASE + 0x1F;
pub const MON_7: nl_item = DATE_BASE + 0x20;
pub const MON_8: nl_item = DATE_BASE + 0x21;
pub const MON_9: nl_item = DATE_BASE + 0x22;
pub const MSG_CMSG_CLOEXEC: c_int = 0x40000000;
pub const MSG_FASTOPEN: c_int = 0x20000000;
pub const NCCS: usize = 32;
pub const NOEXPR: nl_item = 0x50001;
pub const O_APPEND: c_int = 1024;
pub const O_ASYNC: c_int = 0x2000;
pub const O_CLOEXEC: c_int = 0x80000;
pub const O_CREAT: c_int = 64;
pub const O_DSYNC: c_int = 4096;
pub const O_EXCL: c_int = 128;
pub const O_NOATIME: c_int = 0o1000000;
pub const O_NOCTTY: c_int = 256;
pub const O_NONBLOCK: c_int = 2048;
pub const O_PATH: c_int = 0o10000000;
pub const O_RSYNC: c_int = 1052672;
pub const O_SYNC: c_int = 1052672;
pub const O_TMPFILE: c_int = 0o20000000 | O_DIRECTORY;
pub const O_TRUNC: c_int = 512;
pub const PARENB: tcflag_t = 0x00000100;
pub const PIPE_BUF: usize = 4096;
pub const PM_STR: nl_item = DATE_BASE + 0x27;
pub const POSIX_SPAWN_SETPGROUP: c_int = 0x02;
pub const POSIX_SPAWN_SETSID: c_short = 128;
pub const POSIX_SPAWN_SETSIGDEF: c_int = 0x04;
pub const POSIX_SPAWN_SETSIGMASK: c_int = 0x08;
pub const PR_SET_DUMPABLE: c_int = 4;
pub const PR_SET_NAME: c_int = 15;
pub const PR_SET_NO_NEW_PRIVS: c_int = 38;
pub const PTHREAD_CREATE_DETACHED: c_int = 1;
pub const RADIXCHAR: nl_item = 0x10000;
pub const RLIM_INFINITY: rlim_t = !0;
pub const RTLD_DEFAULT: *mut c_void = ptr::null_mut();
pub const RTLD_GLOBAL: c_int = 0x100;
pub const RTLD_LAZY: c_int = 1;
pub const RTLD_LOCAL: c_int = 0;
pub const RTLD_NOW: c_int = 0x2;
pub const SA_ONSTACK: c_int = 0x08000000;
pub const SA_RESTART: c_int = 0x10000000;
pub const SA_SIGINFO: c_int = 0x00000004;
pub const SECCOMP_SET_MODE_FILTER: c_uint = 1;
pub const SEEK_DATA: c_int = 3;
pub const SEEK_HOLE: c_int = 4;
pub const SIGBUS: c_int = 7;
pub const SIGCHLD: c_int = 17;
pub const SIGCONT: c_int = 18;
pub const SIGPWR: c_int = 30;
pub const SIGSTKFLT: c_int = 16;
pub const SIGSTOP: c_int = 19;
pub const SIGSYS: c_int = 31;
pub const SIGTSTP: c_int = 20;
pub const SIGURG: c_int = 23;
pub const SIGUSR1: c_int = 10;
pub const SIGUSR2: c_int = 12;
pub const SIG_SETMASK: c_int = 2;
pub const SOL_SOCKET: c_int = 1;
pub const SO_ERROR: c_int = 4;
pub const SYS_close_range: c_long = 436;
pub const SYS_io_uring_enter: c_long = 426;
pub const SYS_io_uring_register: c_long = 427;
pub const SYS_io_uring_setup: c_long = 425;
pub const SYS_landlock_add_rule: c_long = 445;
pub const SYS_landlock_create_ruleset: c_long = 444;
pub const SYS_landlock_restrict_self: c_long = 446;
pub const SYS_pidfd_open: c_long = 434;
pub const SYS_pidfd_send_signal: c_long = 424;
pub const THOUSEP: nl_item = 0x10001;
pub const TIOCGPTN: Ioctl = u32_cast_ioctl(0x80045430);
pub const TIOCGWINSZ: Ioctl = 0x5413;
pub const TIOCSCTTY: Ioctl = 0x540E;
pub const TIOCSPTLCK: Ioctl = 0x40045431;
pub const TIOCSWINSZ: Ioctl = 0x5414;
pub const T_FMT: nl_item = DATE_BASE + 0x2A;
pub const T_FMT_AMPM: nl_item = DATE_BASE + 0x2B;
pub(super) const ULONG_SIZE: usize = 64;
pub const UTIME_OMIT: c_long = 1073741822;
pub const VEOF: usize = 4;
pub const VMIN: usize = 6;
pub const VTIME: usize = 5;
pub const WCONTINUED: c_int = 0x00000008;
pub const WNOWAIT: c_int = 0x01000000;
pub const WSTOPPED: c_int = WUNTRACED;
pub const WUNTRACED: c_int = 0x00000002;
pub const YESEXPR: nl_item = 0x50000;
pub const _CS_PATH: c_int = 0;
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
pub const _SC_CLK_TCK: c_int = 2;
pub const _SC_GETPW_R_SIZE_MAX: c_int = 70;
pub const _SC_PAGESIZE: c_int = 30;
pub const _SC_THREAD_STACK_MIN: c_int = 75;
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
pub(super) const fn u32_cast_ioctl(x: u32) -> Ioctl {
    x as Ioctl
}
unsafe extern "C" {
    pub fn __errno_location() -> *mut c_int;
    pub fn clock_settime(clk_id: clockid_t, tp: *const timespec) -> c_int;
    pub fn clone(
        cb: extern "C" fn(*mut c_void) -> c_int,
        child_stack: *mut c_void,
        flags: c_int,
        arg: *mut c_void,
        ...
    ) -> c_int;
    pub fn close(fd: c_int) -> c_int;
    pub fn confstr(name: c_int, buf: *mut c_char, len: size_t) -> size_t;
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
    pub fn getloadavg(loadavg: *mut c_double, nelem: c_int) -> c_int;
    pub fn getrandom(buf: *mut c_void, buflen: size_t, flags: c_uint) -> ssize_t;
    pub fn ioctl(fd: c_int, request: Ioctl, ...) -> c_int;
    pub fn mremap(
        addr: *mut c_void,
        len: size_t,
        new_len: size_t,
        flags: c_int,
        ...
    ) -> *mut c_void;
    pub fn nl_langinfo(item: nl_item) -> *mut c_char;
    pub fn posix_spawn(
        pid: *mut pid_t,
        path: *const c_char,
        file_actions: *const posix_spawn_file_actions_t,
        attrp: *const posix_spawnattr_t,
        argv: *const *mut c_char,
        envp: *const *mut c_char,
    ) -> c_int;
    pub fn posix_spawn_file_actions_addchdir_np(
        actions: *mut posix_spawn_file_actions_t,
        path: *const c_char,
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
    pub fn stat(path: *const c_char, buf: *mut stat) -> c_int;
    pub fn strftime(s: *mut c_char, max: size_t, format: *const c_char, tm: *const tm) -> size_t;
    pub fn syscall(num: c_long, ...) -> c_long;
    pub fn system(s: *const c_char) -> c_int;
}
