#[allow(unused_imports)]
use super::*;
pub use core::ffi::c_ulonglong;
pub type Ioctl = c_int;
pub type clock_t = c_long;
pub type fsblkcnt_t = c_ulonglong;
pub type fsfilcnt_t = c_ulonglong;
pub type pthread_t = *mut c_void;
pub type rlim_t = c_ulonglong;
pub type suseconds_t = c_long;
pub type time_t = c_long;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous___first___addr_band {
    pub(super) si_lower: *mut c_void,
    pub(super) si_upper: *mut c_void,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous___first___piduid {
    pub(super) si_pid: pid_t,
    pub(super) si_uid: uid_t,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous___first___timer {
    pub(super) si_timerid: c_int,
    pub(super) si_overrun: c_int,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous___second___sigchld {
    pub(super) si_status: c_int,
    pub(super) si_utime: clock_t,
    pub(super) si_stime: clock_t,
}
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) union __c_anonymous___si_common___first {
    pub(super) __piduid: __c_anonymous___first___piduid,
    pub(super) __timer: __c_anonymous___first___timer,
}
impl ::core::fmt::Debug for __c_anonymous___si_common___first {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("__c_anonymous___si_common___first")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) union __c_anonymous___si_common___second {
    pub(super) si_value: sigval,
    pub(super) __sigchld: __c_anonymous___second___sigchld,
}
impl ::core::fmt::Debug for __c_anonymous___si_common___second {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("__c_anonymous___si_common___second")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous___si_fields___si_common {
    pub(super) __first: __c_anonymous___si_common___first,
    pub(super) __second: __c_anonymous___si_common___second,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous___si_fields___sigfault {
    pub(super) si_addr: *mut c_void,
    pub(super) si_addr_lsb: c_short,
    pub(super) __first: __c_anonymous___sigfault___first,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous___si_fields___sigpoll {
    pub(super) si_band: c_long,
    pub(super) si_fd: c_int,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous___si_fields___sigsys {
    pub(super) si_call_addr: *mut c_void,
    pub(super) si_syscall: c_int,
    pub(super) si_arch: c_uint,
}
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) union __c_anonymous___sigfault___first {
    pub(super) __addr_band: __c_anonymous___first___addr_band,
    pub(super) si_pkey: c_uint,
}
impl ::core::fmt::Debug for __c_anonymous___sigfault___first {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("__c_anonymous___sigfault___first")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) union __c_anonymous_siginfo_t___si_fields {
    pub(super) __pad: [c_char; 128 - 2 * size_of::<c_int>() - size_of::<c_long>()],
    pub(super) __si_common: __c_anonymous___si_fields___si_common,
    pub(super) __sigfault: __c_anonymous___si_fields___sigfault,
    pub(super) __sigpoll: __c_anonymous___si_fields___sigpoll,
    pub(super) __sigsys: __c_anonymous___si_fields___sigsys,
}
impl ::core::fmt::Debug for __c_anonymous_siginfo_t___si_fields {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("__c_anonymous_siginfo_t___si_fields")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct cmsghdr {
    pub cmsg_len: socklen_t,
    pub __pad1: c_int,
    pub cmsg_level: c_int,
    pub cmsg_type: c_int,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct msghdr {
    pub msg_name: *mut c_void,
    pub msg_namelen: socklen_t,
    pub msg_iov: *mut iovec,
    pub msg_iovlen: c_int,
    pub(super) __pad1: Padding<c_int>,
    pub msg_control: *mut c_void,
    pub msg_controllen: socklen_t,
    pub(super) __pad2: Padding<c_int>,
    pub msg_flags: c_int,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct posix_spawnattr_t {
    pub(super) __flags: c_short,
    pub(super) __pgrp: pid_t,
    pub(super) __sd: sigset_t,
    pub(super) __ss: sigset_t,
    pub(super) __prio: c_int,
    pub(super) __policy: c_int,
    pub(super) __pad: Padding<[c_int; 16]>,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct pthread_attr_t {
    pub(super) __size: [u64; 7],
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
    pub(super) __reserved: Padding<[c_long; 16]>,
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
            __reserved: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sched_param {
    pub sched_priority: c_int,
    pub sched_ss_low_priority: c_int,
    pub sched_ss_repl_period: timespec,
    pub sched_ss_init_budget: timespec,
    pub sched_ss_max_repl: c_int,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct siginfo_t {
    pub si_signo: c_int,
    pub si_errno: c_int,
    pub si_code: c_int,
    pub(super) __si_fields: __c_anonymous_siginfo_t___si_fields,
}
impl siginfo_t {
    #[inline]
    pub const unsafe fn si_pid(&self) -> pid_t {
        unsafe { self.__si_fields.__si_common.__first.__piduid.si_pid }
    }
    #[inline]
    pub const unsafe fn si_uid(&self) -> uid_t {
        unsafe { self.__si_fields.__si_common.__first.__piduid.si_uid }
    }
    #[inline]
    pub const unsafe fn si_status(&self) -> c_int {
        unsafe { self.__si_fields.__si_common.__second.__sigchld.si_status }
    }
    #[inline]
    pub const unsafe fn si_utime(&self) -> clock_t {
        unsafe { self.__si_fields.__si_common.__second.__sigchld.si_utime }
    }
    #[inline]
    pub const unsafe fn si_stime(&self) -> clock_t {
        unsafe { self.__si_fields.__si_common.__second.__sigchld.si_stime }
    }
    #[inline]
    pub const unsafe fn si_value(&self) -> sigval {
        unsafe { self.__si_fields.__si_common.__second.si_value }
    }
    #[inline]
    pub const unsafe fn si_addr(&self) -> *mut c_void {
        unsafe { self.__si_fields.__sigfault.si_addr }
    }
    #[inline]
    pub const unsafe fn si_addr_lsb(&self) -> c_short {
        unsafe { self.__si_fields.__sigfault.si_addr_lsb }
    }
    #[inline]
    pub const unsafe fn si_lower(&self) -> *mut c_void {
        unsafe { self.__si_fields.__sigfault.__first.__addr_band.si_lower }
    }
    #[inline]
    pub const unsafe fn si_upper(&self) -> *mut c_void {
        unsafe { self.__si_fields.__sigfault.__first.__addr_band.si_upper }
    }
    #[inline]
    pub const unsafe fn si_pkey(&self) -> c_uint {
        unsafe { self.__si_fields.__sigfault.__first.si_pkey }
    }
    #[inline]
    pub const unsafe fn si_band(&self) -> c_long {
        unsafe { self.__si_fields.__sigpoll.si_band }
    }
    #[inline]
    pub const unsafe fn si_fd(&self) -> c_int {
        unsafe { self.__si_fields.__sigpoll.si_fd }
    }
    #[inline]
    pub const unsafe fn si_timerid(&self) -> c_int {
        unsafe { self.__si_fields.__si_common.__first.__timer.si_timerid }
    }
    #[inline]
    pub const unsafe fn si_overrun(&self) -> c_int {
        unsafe { self.__si_fields.__si_common.__first.__timer.si_overrun }
    }
    #[inline]
    pub const unsafe fn si_ptr(&self) -> *mut c_void {
        unsafe { self.__si_fields.__si_common.__second.si_value.sival_ptr }
    }
    #[inline]
    pub unsafe fn si_int(&self) -> c_int {
        unsafe { self.__si_fields.__si_common.__second.si_value.sival_ptr as usize as c_int }
    }
    #[inline]
    pub const unsafe fn si_call_addr(&self) -> *mut c_void {
        unsafe { self.__si_fields.__sigsys.si_call_addr }
    }
    #[inline]
    pub const unsafe fn si_syscall(&self) -> c_int {
        unsafe { self.__si_fields.__sigsys.si_syscall }
    }
    #[inline]
    pub const unsafe fn si_arch(&self) -> c_uint {
        unsafe { self.__si_fields.__sigsys.si_arch }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sigset_t {
    pub(super) __val: [c_ulong; 16],
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
    pub(super) __f_reserved: Padding<[c_int; 6]>,
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
    pub __c_ispeed: speed_t,
    pub __c_ospeed: speed_t,
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
pub const EDEADLOCK: c_int = EDEADLK;
pub const EFD_NONBLOCK: c_int = O_NONBLOCK;
pub const MAP_ANON: c_int = 0x0020;
pub const MAP_ANONYMOUS: c_int = MAP_ANON;
pub const O_ACCMODE: c_int = 0o10000003;
pub const O_EXEC: c_int = 0o10000000;
pub const O_NDELAY: c_int = O_NONBLOCK;
pub const O_SEARCH: c_int = 0o10000000;
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
pub const RLIMIT_RTTIME: c_int = 15;
pub const RLIMIT_SIGPENDING: c_int = 11;
pub const RLIMIT_STACK: c_int = 3;
pub const SHUT_RD: c_int = 0;
pub const SHUT_RDWR: c_int = 2;
pub const SHUT_WR: c_int = 1;
pub const SIGIO: c_int = 29;
pub const SIG_BLOCK: c_int = 0x000000;
pub const SIG_UNBLOCK: c_int = 0x01;
pub const SOCK_CLOEXEC: c_int = 0o2000000;
pub const SOCK_DGRAM: c_int = 2;
pub const SOCK_NONBLOCK: c_int = 0o4000;
pub const SOCK_STREAM: c_int = 1;
#[inline]
pub unsafe extern "C" fn CMSG_NXTHDR(mhdr: *const msghdr, cmsg: *const cmsghdr) -> *mut cmsghdr {
    unsafe {
        if ((*cmsg).cmsg_len as usize) < size_of::<cmsghdr>() {
            return ptr::null_mut();
        }
        let next_cmsg = cmsg
            .cast::<u8>()
            .wrapping_add(CMSG_ALIGN((*cmsg).cmsg_len as usize))
            .cast::<cmsghdr>();
        if (next_cmsg as usize) < (cmsg as usize) {
            return ptr::null_mut();
        }
        let mut max_addr = (*mhdr).msg_control as usize + (*mhdr).msg_controllen as usize;
        if true {
            max_addr -= 1;
        }
        if next_cmsg as usize + size_of::<cmsghdr>() > max_addr {
            ptr::null_mut()
        } else {
            next_cmsg as *mut cmsghdr
        }
    }
}
unsafe extern "C" {
    pub fn getrlimit(resource: c_int, rlim: *mut rlimit) -> c_int;
    pub fn setrlimit(resource: c_int, rlim: *const rlimit) -> c_int;
}
