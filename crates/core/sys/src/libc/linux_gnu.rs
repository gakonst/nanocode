#[allow(unused_imports)]
use super::*;
pub type Ioctl = c_ulong;
pub(super) type __SI_BAND_TYPE = c_long;
pub type __rlimit_resource_t = c_uint;
pub type clock_t = i64;
pub type fsblkcnt_t = u64;
pub type fsfilcnt_t = u64;
pub type pthread_t = c_ulong;
pub type rlim_t = u64;
pub type suseconds_t = i64;
pub type time_t = i64;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __SI_CLOCK_T(pub(super) clock_t);
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous__bounds__addr_bnd {
    pub(super) _lower: *mut c_void,
    pub(super) _upper: *mut c_void,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous__si_fields__kill {
    pub(super) si_pid: pid_t,
    pub(super) si_uid: uid_t,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous__si_fields__rt {
    pub(super) si_pid: pid_t,
    pub(super) si_uid: uid_t,
    pub(super) si_sigval: sigval,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous__si_fields__sigchld {
    pub(super) si_pid: pid_t,
    pub(super) si_uid: uid_t,
    pub(super) si_status: c_int,
    pub(super) si_utime: __SI_CLOCK_T,
    pub(super) si_stime: __SI_CLOCK_T,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous__si_fields__sigfault {
    pub(super) si_addr: *mut c_void,
    pub(super) si_addr_lsb: c_short,
    pub(super) _bounds: __c_anonymous__sigfault__bounds,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous__si_fields__sigpoll {
    pub(super) si_band: __SI_BAND_TYPE,
    pub(super) si_fd: c_int,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous__si_fields__sigsys {
    pub(super) _call_addr: *mut c_void,
    pub(super) _syscall: c_int,
    pub(super) _arch: c_uint,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct __c_anonymous__si_fields__timer {
    pub(super) si_tid: c_int,
    pub(super) si_overrun: c_int,
    pub(super) si_sigval: sigval,
}
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) union __c_anonymous__sigfault__bounds {
    pub(super) _addr_bnd: __c_anonymous__bounds__addr_bnd,
    pub(super) _pkey: u32,
}
impl ::core::fmt::Debug for __c_anonymous__sigfault__bounds {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("__c_anonymous__sigfault__bounds")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) union __c_anonymous_siginfo_t__si_fields {
    pub(super) _pad: [c_int; __SI_PAD_SIZE],
    pub(super) _kill: __c_anonymous__si_fields__kill,
    pub(super) _timer: __c_anonymous__si_fields__timer,
    pub(super) _rt: __c_anonymous__si_fields__rt,
    pub(super) _sigchld: __c_anonymous__si_fields__sigchld,
    pub(super) _sigfault: __c_anonymous__si_fields__sigfault,
    pub(super) _sigpoll: __c_anonymous__si_fields__sigpoll,
    pub(super) _sigsys: __c_anonymous__si_fields__sigsys,
}
impl ::core::fmt::Debug for __c_anonymous_siginfo_t__si_fields {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("__c_anonymous_siginfo_t__si_fields")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct cmsghdr {
    pub cmsg_len: size_t,
    pub cmsg_level: c_int,
    pub cmsg_type: c_int,
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
pub struct posix_spawnattr_t {
    pub(super) __flags: c_short,
    pub(super) __pgrp: pid_t,
    pub(super) __sd: sigset_t,
    pub(super) __ss: sigset_t,
    pub(super) __sp: sched_param,
    pub(super) __policy: c_int,
    pub(super) __pad: Padding<[c_int; 16]>,
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
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct siginfo_t {
    pub si_signo: c_int,
    pub si_errno: c_int,
    pub si_code: c_int,
    pub(super) __pad0: Padding<c_int>,
    pub(super) _sifields: __c_anonymous_siginfo_t__si_fields,
}
impl siginfo_t {
    #[inline]
    pub const unsafe fn si_pid(&self) -> pid_t {
        unsafe { self._sifields._kill.si_pid }
    }
    #[inline]
    pub const unsafe fn si_uid(&self) -> uid_t {
        unsafe { self._sifields._kill.si_uid }
    }
    #[inline]
    pub const unsafe fn si_timerid(&self) -> c_int {
        unsafe { self._sifields._timer.si_tid }
    }
    #[inline]
    pub const unsafe fn si_overrun(&self) -> c_int {
        unsafe { self._sifields._timer.si_overrun }
    }
    #[inline]
    pub const unsafe fn si_status(&self) -> c_int {
        unsafe { self._sifields._sigchld.si_status }
    }
    #[inline]
    pub const unsafe fn si_utime(&self) -> clock_t {
        unsafe { self._sifields._sigchld.si_utime.0 }
    }
    #[inline]
    pub const unsafe fn si_stime(&self) -> clock_t {
        unsafe { self._sifields._sigchld.si_stime.0 }
    }
    #[inline]
    pub const unsafe fn si_value(&self) -> sigval {
        unsafe { self._sifields._rt.si_sigval }
    }
    #[inline]
    pub unsafe fn si_int(&self) -> c_int {
        unsafe { self._sifields._rt.si_sigval.sival_ptr as usize as c_int }
    }
    #[inline]
    pub const unsafe fn si_ptr(&self) -> *mut c_void {
        unsafe { self._sifields._rt.si_sigval.sival_ptr }
    }
    #[inline]
    pub const unsafe fn si_addr(&self) -> *mut c_void {
        unsafe { self._sifields._sigfault.si_addr }
    }
    #[inline]
    pub const unsafe fn si_addr_lsb(&self) -> c_short {
        unsafe { self._sifields._sigfault.si_addr_lsb }
    }
    #[inline]
    pub const unsafe fn si_lower(&self) -> *mut c_void {
        unsafe { self._sifields._sigfault._bounds._addr_bnd._lower }
    }
    #[inline]
    pub const unsafe fn si_upper(&self) -> *mut c_void {
        unsafe { self._sifields._sigfault._bounds._addr_bnd._upper }
    }
    #[inline]
    pub const unsafe fn si_pkey(&self) -> u32 {
        unsafe { self._sifields._sigfault._bounds._pkey }
    }
    #[inline]
    pub const unsafe fn si_band(&self) -> __SI_BAND_TYPE {
        unsafe { self._sifields._sigpoll.si_band }
    }
    #[inline]
    pub const unsafe fn si_fd(&self) -> c_int {
        unsafe { self._sifields._sigpoll.si_fd }
    }
    #[inline]
    pub const unsafe fn si_call_addr(&self) -> *mut c_void {
        unsafe { self._sifields._sigsys._call_addr }
    }
    #[inline]
    pub const unsafe fn si_syscall(&self) -> c_int {
        unsafe { self._sifields._sigsys._syscall }
    }
    #[inline]
    pub const unsafe fn si_arch(&self) -> c_uint {
        unsafe { self._sifields._sigsys._arch }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sigset_t {
    pub(super) __val: [u64; 16],
}
#[derive(Clone, Copy, Debug)]
#[allow(clippy::manual_non_exhaustive)]
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
    pub f_type: c_uint,
    pub(super) __f_spare: Padding<[c_int; 5]>,
    pub(super) __non_exhaustive: (),
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
            f_type: ::core::default::Default::default(),
            __f_spare: ::core::default::Default::default(),
            __non_exhaustive: (),
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
    pub c_line: cc_t,
    pub c_cc: [cc_t; NCCS],
    pub c_ispeed: speed_t,
    pub c_ospeed: speed_t,
}
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct timespec {
    pub tv_sec: time_t,
    pub tv_nsec: c_long,
}
pub const EDEADLOCK: c_int = 35;
pub const EFD_NONBLOCK: c_int = 0x800;
pub const MAP_ANONYMOUS: c_int = 0x0020;
pub const O_ACCMODE: c_int = 3;
pub const O_FSYNC: c_int = 0x101000;
pub const O_LARGEFILE: c_int = 0;
pub const O_NDELAY: c_int = 0x800;
pub const RLIMIT_AS: __rlimit_resource_t = 9;
pub const RLIMIT_CORE: __rlimit_resource_t = 4;
pub const RLIMIT_CPU: __rlimit_resource_t = 0;
pub const RLIMIT_DATA: __rlimit_resource_t = 2;
pub const RLIMIT_FSIZE: __rlimit_resource_t = 1;
pub const RLIMIT_LOCKS: __rlimit_resource_t = 10;
pub const RLIMIT_MEMLOCK: __rlimit_resource_t = 8;
pub const RLIMIT_MSGQUEUE: __rlimit_resource_t = 12;
pub const RLIMIT_NICE: __rlimit_resource_t = 13;
pub const RLIMIT_NOFILE: __rlimit_resource_t = 7;
pub const RLIMIT_NPROC: __rlimit_resource_t = 6;
pub const RLIMIT_RSS: __rlimit_resource_t = 5;
pub const RLIMIT_RTPRIO: __rlimit_resource_t = 14;
pub const RLIMIT_RTTIME: __rlimit_resource_t = 15;
pub const RLIMIT_SIGPENDING: __rlimit_resource_t = 11;
pub const RLIMIT_STACK: __rlimit_resource_t = 3;
pub const SHUT_RD: c_int = {
    #[allow(unused_variables)]
    let r = 0;
    let r = 0;
    r
};
pub const SHUT_RDWR: c_int = {
    #[allow(unused_variables)]
    let r = SHUT_WR + 1;
    r
};
pub const SHUT_WR: c_int = {
    #[allow(unused_variables)]
    let r = SHUT_RD + 1;
    r
};
pub const SIGIO: c_int = SIGPOLL;
pub const SIGPOLL: c_int = 29;
pub const SIG_BLOCK: c_int = 0;
pub const SIG_UNBLOCK: c_int = 1;
pub const SOCK_CLOEXEC: c_int = {
    #[allow(unused_variables)]
    let r = SOCK_PACKET + 1;
    let r = 0o2000000;
    r
};
pub const SOCK_DCCP: c_int = {
    #[allow(unused_variables)]
    let r = SOCK_SEQPACKET + 1;
    let r = 6;
    r
};
pub const SOCK_DGRAM: c_int = {
    #[allow(unused_variables)]
    let r = SOCK_STREAM + 1;
    let r = 2;
    r
};
pub const SOCK_NONBLOCK: c_int = {
    #[allow(unused_variables)]
    let r = SOCK_CLOEXEC + 1;
    let r = 0o0004000;
    r
};
pub const SOCK_PACKET: c_int = {
    #[allow(unused_variables)]
    let r = SOCK_DCCP + 1;
    let r = 10;
    r
};
pub const SOCK_RAW: c_int = {
    #[allow(unused_variables)]
    let r = SOCK_DGRAM + 1;
    let r = 3;
    r
};
pub const SOCK_RDM: c_int = {
    #[allow(unused_variables)]
    let r = SOCK_RAW + 1;
    let r = 4;
    r
};
pub const SOCK_SEQPACKET: c_int = {
    #[allow(unused_variables)]
    let r = SOCK_RDM + 1;
    let r = 5;
    r
};
pub const SOCK_STREAM: c_int = {
    #[allow(unused_variables)]
    let r = 0;
    let r = 1;
    r
};
pub(super) const __SI_MAX_SIZE: usize = 128;
pub(super) const __SI_PAD_SIZE: usize = if true {
    __SI_MAX_SIZE / size_of::<c_int>() - 4
} else {
    __SI_MAX_SIZE / size_of::<c_int>() - 3
};
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
        if false {
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
    pub fn getentropy(buf: *mut c_void, buflen: size_t) -> c_int;
    pub fn getrlimit(resource: __rlimit_resource_t, rlim: *mut rlimit) -> c_int;
    pub fn setrlimit(resource: __rlimit_resource_t, rlim: *const rlimit) -> c_int;
}
