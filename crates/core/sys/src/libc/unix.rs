#[allow(unused_imports)]
use super::*;
pub use core::ffi::c_char;
pub use core::ffi::c_int;
pub use core::ffi::c_long;
pub use core::ffi::c_longlong;
pub use core::ffi::c_short;
pub use core::ffi::c_uchar;
pub use core::ffi::c_uint;
pub use core::ffi::c_ulong;
pub use core::ffi::c_ushort;
pub use core::ffi::c_void;
pub type cc_t = c_uchar;
pub type gid_t = u32;
pub type id_t = c_uint;
pub type in_addr_t = u32;
pub type in_port_t = u16;
pub type intptr_t = isize;
pub type pid_t = i32;
pub type sighandler_t = size_t;
pub type size_t = usize;
pub type socklen_t = u32;
pub type ssize_t = isize;
pub type uid_t = u32;
pub type uintptr_t = usize;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct FILE {
    pub(super) _data: (),
    pub(super) _marker: ::core::marker::PhantomData<(*mut u8, ::core::marker::PhantomPinned)>,
}
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(super) struct Padding<T: Copy>(pub(super) core::mem::MaybeUninit<T>);
impl<T: Copy> Default for Padding<T> {
    fn default() -> Self {
        Self(core::mem::MaybeUninit::zeroed())
    }
}
impl<T: Copy> Padding<T> {
    pub(super) const fn new(val: T) -> Self {
        Self(core::mem::MaybeUninit::new(val))
    }
}
impl<T: Copy> core::fmt::Debug for Padding<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let full_name = core::any::type_name::<Self>();
        let prefix_len = full_name.find("Padding").unwrap();
        f.pad(&full_name[prefix_len..])
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct group {
    pub gr_name: *mut c_char,
    pub gr_passwd: *mut c_char,
    pub gr_gid: gid_t,
    pub gr_mem: *mut *mut c_char,
}
impl ::core::default::Default for group {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            gr_name: ptr::null_mut(),
            gr_passwd: ptr::null_mut(),
            gr_gid: ::core::default::Default::default(),
            gr_mem: ptr::null_mut(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
#[repr(align(4))]
pub struct in6_addr {
    pub s6_addr: [u8; 16],
}
impl ::core::default::Default for in6_addr {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            s6_addr: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct iovec {
    pub iov_base: *mut c_void,
    pub iov_len: size_t,
}
impl ::core::default::Default for iovec {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            iov_base: ptr::null_mut(),
            iov_len: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct itimerval {
    pub it_interval: timeval,
    pub it_value: timeval,
}
impl ::core::default::Default for itimerval {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            it_interval: ::core::default::Default::default(),
            it_value: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct pollfd {
    pub fd: c_int,
    pub events: c_short,
    pub revents: c_short,
}
impl ::core::default::Default for pollfd {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            fd: ::core::default::Default::default(),
            events: ::core::default::Default::default(),
            revents: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct protoent {
    pub p_name: *mut c_char,
    pub p_aliases: *mut *mut c_char,
    pub p_proto: c_int,
}
impl ::core::default::Default for protoent {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            p_name: ptr::null_mut(),
            p_aliases: ptr::null_mut(),
            p_proto: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct rlimit {
    pub rlim_cur: rlim_t,
    pub rlim_max: rlim_t,
}
impl ::core::default::Default for rlimit {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            rlim_cur: ::core::default::Default::default(),
            rlim_max: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct servent {
    pub s_name: *mut c_char,
    pub s_aliases: *mut *mut c_char,
    pub s_port: c_int,
    pub s_proto: *mut c_char,
}
impl ::core::default::Default for servent {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            s_name: ptr::null_mut(),
            s_aliases: ptr::null_mut(),
            s_port: ::core::default::Default::default(),
            s_proto: ptr::null_mut(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sigval {
    pub sival_ptr: *mut c_void,
}
impl ::core::default::Default for sigval {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            sival_ptr: ptr::null_mut(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct timeval {
    pub tv_sec: time_t,
    pub tv_usec: suseconds_t,
}
impl ::core::default::Default for timeval {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            tv_sec: ::core::default::Default::default(),
            tv_usec: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct tms {
    pub tms_utime: clock_t,
    pub tms_stime: clock_t,
    pub tms_cutime: clock_t,
    pub tms_cstime: clock_t,
}
impl ::core::default::Default for tms {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            tms_utime: ::core::default::Default::default(),
            tms_stime: ::core::default::Default::default(),
            tms_cutime: ::core::default::Default::default(),
            tms_cstime: ::core::default::Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct winsize {
    pub ws_row: c_ushort,
    pub ws_col: c_ushort,
    pub ws_xpixel: c_ushort,
    pub ws_ypixel: c_ushort,
}
impl ::core::default::Default for winsize {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            ws_row: ::core::default::Default::default(),
            ws_col: ::core::default::Default::default(),
            ws_xpixel: ::core::default::Default::default(),
            ws_ypixel: ::core::default::Default::default(),
        }
    }
}
pub const AF_INET: c_int = 2;
pub const AF_UNSPEC: c_int = 0;
pub const BRKINT: tcflag_t = 0x00000002;
pub const CLD_DUMPED: c_int = 3;
pub const CLD_EXITED: c_int = 1;
pub const CLD_KILLED: c_int = 2;
pub const CLD_STOPPED: c_int = 5;
pub const CLOCK_MONOTONIC_RAW: clockid_t = 4;
pub const CLOCK_REALTIME: clockid_t = 0;
pub const DT_DIR: u8 = 4;
pub const DT_LNK: u8 = 10;
pub const DT_REG: u8 = 8;
pub const DT_UNKNOWN: u8 = 0;
pub const E2BIG: c_int = 7;
pub const EACCES: c_int = 13;
pub const EBADF: c_int = 9;
pub const EBUSY: c_int = 16;
pub const ECHILD: c_int = 10;
pub const ECHO: tcflag_t = 0x00000008;
pub const EDOM: c_int = 33;
pub const EEXIST: c_int = 17;
pub const EFAULT: c_int = 14;
pub const EFBIG: c_int = 27;
pub const EINTR: c_int = 4;
pub const EINVAL: c_int = 22;
pub const EIO: c_int = 5;
pub const EISDIR: c_int = 21;
pub const EMFILE: c_int = 24;
pub const EMLINK: c_int = 31;
pub const ENFILE: c_int = 23;
pub const ENODEV: c_int = 19;
pub const ENOENT: c_int = 2;
pub const ENOEXEC: c_int = 8;
pub const ENOMEM: c_int = 12;
pub const ENOSPC: c_int = 28;
pub const ENOTBLK: c_int = 15;
pub const ENOTDIR: c_int = 20;
pub const ENOTTY: c_int = 25;
pub const ENXIO: c_int = 6;
pub const EPERM: c_int = 1;
pub const EPIPE: c_int = 32;
pub const ERANGE: c_int = 34;
pub const EROFS: c_int = 30;
pub const ESPIPE: c_int = 29;
pub const ESRCH: c_int = 3;
pub const ETXTBSY: c_int = 26;
pub const EWOULDBLOCK: c_int = EAGAIN;
pub const EXDEV: c_int = 18;
pub const FD_CLOEXEC: c_int = 0x1;
pub const FD_SETSIZE: usize = 1024;
pub const F_GETFD: c_int = 1;
pub const F_GETFL: c_int = 3;
pub const F_OK: c_int = 0;
pub const F_SETFD: c_int = 2;
pub const F_SETFL: c_int = 4;
pub const ICRNL: tcflag_t = 0x00000100;
pub const IF_NAMESIZE: size_t = 16;
pub const IGNBRK: tcflag_t = 0x00000001;
pub const IGNCR: tcflag_t = 0x00000080;
pub const INADDR_BROADCAST: in_addr_t = 4294967295;
pub const INLCR: tcflag_t = 0x00000040;
pub const IPPROTO_TCP: c_int = 6;
pub const ISTRIP: tcflag_t = 0x00000020;
pub const MADV_DONTNEED: c_int = 4;
pub const MAP_FAILED: *mut c_void = !0 as *mut c_void;
pub const MAP_PRIVATE: c_int = 0x0002;
pub const OPOST: tcflag_t = 0x1;
pub const O_RDONLY: c_int = 0;
pub const O_RDWR: c_int = 2;
pub const O_WRONLY: c_int = 1;
pub const PARMRK: tcflag_t = 0x00000008;
pub const POLLIN: c_short = 0x1;
pub const POLLOUT: c_short = 0x4;
pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const P_PID: idtype_t = 1;
pub const RUSAGE_CHILDREN: c_int = -1;
pub const RUSAGE_SELF: c_int = 0;
pub const R_OK: c_int = 4;
pub const SCM_RIGHTS: c_int = 0x01;
pub const SEEK_CUR: c_int = 1;
pub const SEEK_END: c_int = 2;
pub const SEEK_SET: c_int = 0;
pub const SIGABRT: c_int = 6;
pub const SIGALRM: c_int = 14;
pub const SIGFPE: c_int = 8;
pub const SIGHUP: c_int = 1;
pub const SIGILL: c_int = 4;
pub const SIGINT: c_int = 2;
pub const SIGKILL: c_int = 9;
pub const SIGPIPE: c_int = 13;
pub const SIGPROF: c_int = 27;
pub const SIGQUIT: c_int = 3;
pub const SIGSEGV: c_int = 11;
pub const SIGTERM: c_int = 15;
pub const SIGTRAP: c_int = 5;
pub const SIGTTIN: c_int = 21;
pub const SIGTTOU: c_int = 22;
pub const SIGVTALRM: c_int = 26;
pub const SIGWINCH: c_int = 28;
pub const SIGXCPU: c_int = 24;
pub const SIGXFSZ: c_int = 25;
pub const SIG_DFL: sighandler_t = 0 as sighandler_t;
pub const SIG_ERR: sighandler_t = !0 as sighandler_t;
pub const SIG_IGN: sighandler_t = 1 as sighandler_t;
pub const STDERR_FILENO: c_int = 2;
pub const STDIN_FILENO: c_int = 0;
pub const STDOUT_FILENO: c_int = 1;
pub const ST_NOSUID: c_ulong = 2;
pub const ST_RDONLY: c_ulong = 1;
pub const S_IFBLK: mode_t = 0o6_0000;
pub const S_IFCHR: mode_t = 0o2_0000;
pub const S_IFDIR: mode_t = 0o4_0000;
pub const S_IFIFO: mode_t = 0o1_0000;
pub const S_IFLNK: mode_t = 0o12_0000;
pub const S_IFMT: mode_t = 0o17_0000;
pub const S_IFREG: mode_t = 0o10_0000;
pub const S_IFSOCK: mode_t = 0o14_0000;
pub const TCSANOW: c_int = 0;
pub const WEXITED: c_int = 0x00000004;
pub const WNOHANG: c_int = 0x00000001;
pub const W_OK: c_int = 2;
pub const X_OK: c_int = 1;
#[inline]
pub unsafe extern "C" fn CMSG_FIRSTHDR(mhdr: *const msghdr) -> *mut cmsghdr {
    unsafe {
        if (*mhdr).msg_controllen as usize >= size_of::<cmsghdr>() {
            (*mhdr).msg_control.cast()
        } else {
            ptr::null_mut()
        }
    }
}
#[inline]
pub unsafe extern "C" fn FD_ZERO(set: *mut fd_set) -> () {
    unsafe {
        (*set).fds_bits.fill(0);
    }
}
unsafe extern "C" {
    pub fn _exit(status: c_int) -> !;
    pub fn abort() -> !;
    pub fn access(path: *const c_char, amode: c_int) -> c_int;
    pub fn alarm(seconds: c_uint) -> c_uint;
    pub fn atexit(cb: extern "C" fn()) -> c_int;
    pub fn bind(socket: c_int, address: *const sockaddr, address_len: socklen_t) -> c_int;
    pub fn chdir(dir: *const c_char) -> c_int;
    pub fn clock_getres(clk_id: clockid_t, tp: *mut timespec) -> c_int;
    pub fn clock_gettime(clk_id: clockid_t, tp: *mut timespec) -> c_int;
    pub fn connect(socket: c_int, address: *const sockaddr, len: socklen_t) -> c_int;
    pub fn dlerror() -> *mut c_char;
    pub fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    pub fn dup(fd: c_int) -> c_int;
    pub fn dup2(src: c_int, dst: c_int) -> c_int;
    pub fn execve(
        prog: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> c_int;
    pub fn exit(status: c_int) -> !;
    pub fn faccessat(dirfd: c_int, pathname: *const c_char, mode: c_int, flags: c_int) -> c_int;
    pub fn fclose(file: *mut FILE) -> c_int;
    pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    pub fn fdopen(fd: c_int, mode: *const c_char) -> *mut FILE;
    pub fn fork() -> pid_t;
    pub fn fsync(fd: c_int) -> c_int;
    pub fn ftruncate(fd: c_int, length: off_t) -> c_int;
    pub fn geteuid() -> uid_t;
    pub fn getgid() -> gid_t;
    pub fn getgrgid_r(
        gid: gid_t,
        grp: *mut group,
        buf: *mut c_char,
        buflen: size_t,
        result: *mut *mut group,
    ) -> c_int;
    pub fn getgroups(ngroups_max: c_int, groups: *mut gid_t) -> c_int;
    pub fn gethostname(name: *mut c_char, len: size_t) -> c_int;
    pub fn getitimer(which: c_int, curr_value: *mut itimerval) -> c_int;
    pub fn getpgrp() -> pid_t;
    pub fn getpid() -> pid_t;
    pub fn getppid() -> pid_t;
    pub fn getprotobyname(name: *const c_char) -> *mut protoent;
    pub fn getpwnam_r(
        name: *const c_char,
        pwd: *mut passwd,
        buf: *mut c_char,
        buflen: size_t,
        result: *mut *mut passwd,
    ) -> c_int;
    pub fn getpwuid_r(
        uid: uid_t,
        pwd: *mut passwd,
        buf: *mut c_char,
        buflen: size_t,
        result: *mut *mut passwd,
    ) -> c_int;
    pub fn getrusage(resource: c_int, usage: *mut rusage) -> c_int;
    pub fn getservbyname(name: *const c_char, proto: *const c_char) -> *mut servent;
    pub fn getservbyport(port: c_int, proto: *const c_char) -> *mut servent;
    pub fn getsid(pid: pid_t) -> pid_t;
    pub fn getsockopt(
        sockfd: c_int,
        level: c_int,
        optname: c_int,
        optval: *mut c_void,
        optlen: *mut socklen_t,
    ) -> c_int;
    pub fn getuid() -> uid_t;
    pub fn gmtime_r(time_p: *const time_t, result: *mut tm) -> *mut tm;
    pub fn grantpt(fd: c_int) -> c_int;
    pub fn if_indextoname(ifindex: c_uint, ifname: *mut c_char) -> *mut c_char;
    pub fn if_nametoindex(ifname: *const c_char) -> c_uint;
    pub fn isatty(fd: c_int) -> c_int;
    pub fn kill(pid: pid_t, sig: c_int) -> c_int;
    pub fn killpg(pgrp: pid_t, sig: c_int) -> c_int;
    pub fn linkat(
        olddirfd: c_int,
        oldpath: *const c_char,
        newdirfd: c_int,
        newpath: *const c_char,
        flags: c_int,
    ) -> c_int;
    pub fn localeconv() -> *mut lconv;
    pub fn localtime_r(time_p: *const time_t, result: *mut tm) -> *mut tm;
    pub fn lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t;
    pub fn madvise(addr: *mut c_void, len: size_t, advice: c_int) -> c_int;
    pub fn memmove(dest: *mut c_void, src: *const c_void, n: size_t) -> *mut c_void;
    pub fn memset(dest: *mut c_void, c: c_int, n: size_t) -> *mut c_void;
    pub fn mkfifo(path: *const c_char, mode: mode_t) -> c_int;
    pub fn mktime(tm: *mut tm) -> time_t;
    pub fn mmap(
        addr: *mut c_void,
        len: size_t,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: off_t,
    ) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, len: size_t) -> c_int;
    pub fn nanosleep(rqtp: *const timespec, rmtp: *mut timespec) -> c_int;
    pub fn open(path: *const c_char, oflag: c_int, ...) -> c_int;
    pub fn openat(dirfd: c_int, pathname: *const c_char, flags: c_int, ...) -> c_int;
    pub fn pause() -> c_int;
    pub fn pipe(fds: *mut c_int) -> c_int;
    pub fn poll(fds: *mut pollfd, nfds: nfds_t, timeout: c_int) -> c_int;
    pub fn posix_openpt(flags: c_int) -> c_int;
    pub fn pthread_atfork(
        prepare: Option<unsafe extern "C" fn()>,
        parent: Option<unsafe extern "C" fn()>,
        child: Option<unsafe extern "C" fn()>,
    ) -> c_int;
    pub fn pthread_attr_destroy(attr: *mut pthread_attr_t) -> c_int;
    pub fn pthread_attr_getstacksize(attr: *const pthread_attr_t, stacksize: *mut size_t) -> c_int;
    pub fn pthread_attr_init(attr: *mut pthread_attr_t) -> c_int;
    pub fn pthread_attr_setdetachstate(attr: *mut pthread_attr_t, state: c_int) -> c_int;
    pub fn pthread_attr_setstacksize(attr: *mut pthread_attr_t, stack_size: size_t) -> c_int;
    pub fn pthread_create(
        native: *mut pthread_t,
        attr: *const pthread_attr_t,
        f: extern "C" fn(*mut c_void) -> *mut c_void,
        value: *mut c_void,
    ) -> c_int;
    pub fn pthread_key_create(
        key: *mut pthread_key_t,
        dtor: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_int;
    pub fn pthread_key_delete(key: pthread_key_t) -> c_int;
    pub fn pthread_self() -> pthread_t;
    pub fn pthread_setspecific(key: pthread_key_t, value: *const c_void) -> c_int;
    pub fn pthread_sigmask(how: c_int, set: *const sigset_t, oldset: *mut sigset_t) -> c_int;
    pub fn raise(signum: c_int) -> c_int;
    pub fn read(fd: c_int, buf: *mut c_void, count: size_t) -> ssize_t;
    pub fn recvmsg(fd: c_int, msg: *mut msghdr, flags: c_int) -> ssize_t;
    pub fn rename(oldname: *const c_char, newname: *const c_char) -> c_int;
    pub fn renameat(
        olddirfd: c_int,
        oldpath: *const c_char,
        newdirfd: c_int,
        newpath: *const c_char,
    ) -> c_int;
    pub fn sendmsg(fd: c_int, msg: *const msghdr, flags: c_int) -> ssize_t;
    pub fn setitimer(which: c_int, new_value: *const itimerval, old_value: *mut itimerval)
    -> c_int;
    pub fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
    pub fn setpgid(pid: pid_t, pgid: pid_t) -> c_int;
    pub fn setsid() -> pid_t;
    pub fn setsockopt(
        socket: c_int,
        level: c_int,
        name: c_int,
        value: *const c_void,
        option_len: socklen_t,
    ) -> c_int;
    pub fn sigaction(signum: c_int, act: *const sigaction, oldact: *mut sigaction) -> c_int;
    pub fn sigaddset(set: *mut sigset_t, signum: c_int) -> c_int;
    pub fn sigdelset(set: *mut sigset_t, signum: c_int) -> c_int;
    pub fn sigemptyset(set: *mut sigset_t) -> c_int;
    pub fn sigfillset(set: *mut sigset_t) -> c_int;
    pub fn sigismember(set: *const sigset_t, signum: c_int) -> c_int;
    pub fn signal(signum: c_int, handler: sighandler_t) -> sighandler_t;
    pub fn sigprocmask(how: c_int, set: *const sigset_t, oldset: *mut sigset_t) -> c_int;
    pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
    pub fn statvfs(path: *const c_char, buf: *mut statvfs) -> c_int;
    pub fn strcoll(cs: *const c_char, ct: *const c_char) -> c_int;
    pub fn strerror(n: c_int) -> *mut c_char;
    pub fn strsignal(sig: c_int) -> *mut c_char;
    pub fn strxfrm(s: *mut c_char, ct: *const c_char, n: size_t) -> size_t;
    pub fn sysconf(name: c_int) -> c_long;
    pub fn tcgetattr(fd: c_int, termios: *mut termios) -> c_int;
    pub fn tcsetattr(fd: c_int, optional_actions: c_int, termios: *const termios) -> c_int;
    pub fn time(time: *mut time_t) -> time_t;
    pub fn times(buf: *mut tms) -> clock_t;
    pub fn truncate(path: *const c_char, length: off_t) -> c_int;
    pub fn umask(mask: mode_t) -> mode_t;
    pub fn uname(buf: *mut utsname) -> c_int;
    pub fn unlockpt(fd: c_int) -> c_int;
    pub fn utimensat(
        dirfd: c_int,
        path: *const c_char,
        times: *const timespec,
        flag: c_int,
    ) -> c_int;
    pub fn waitid(idtype: idtype_t, id: id_t, infop: *mut siginfo_t, options: c_int) -> c_int;
    pub fn waitpid(pid: pid_t, status: *mut c_int, options: c_int) -> pid_t;
    pub fn write(fd: c_int, buf: *const c_void, count: size_t) -> ssize_t;
}
