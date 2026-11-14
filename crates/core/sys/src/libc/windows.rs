#[allow(unused_imports)]
use super::*;
pub use core::ffi::c_char;
pub use core::ffi::c_int;
pub use core::ffi::c_long;
pub use core::ffi::c_longlong;
pub use core::ffi::c_short;
pub use core::ffi::c_uint;
pub use core::ffi::c_ulong;
pub use core::ffi::c_ushort;
pub use core::ffi::c_void;
pub type SOCKET = uintptr_t;
pub type clock_t = i32;
pub type dev_t = u32;
pub type ino_t = u16;
pub type intptr_t = isize;
pub type off_t = i32;
pub type sighandler_t = usize;
pub type size_t = usize;
pub type time64_t = i64;
pub type time_t = i64;
pub type uintptr_t = usize;
pub type wchar_t = u16;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct FILE {
    pub(super) _data: (),
    pub(super) _marker: ::core::marker::PhantomData<(*mut u8, ::core::marker::PhantomPinned)>,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct sockaddr {
    pub sa_family: c_ushort,
    pub sa_data: [c_char; 14],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct stat {
    pub st_dev: dev_t,
    pub st_ino: ino_t,
    pub st_mode: u16,
    pub st_nlink: c_short,
    pub st_uid: c_short,
    pub st_gid: c_short,
    pub st_rdev: dev_t,
    pub st_size: i64,
    pub st_atime: time64_t,
    pub st_mtime: time64_t,
    pub st_ctime: time64_t,
}
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct timespec {
    pub tv_sec: time_t,
    pub tv_nsec: c_long,
}
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct timeval {
    pub tv_sec: c_long,
    pub tv_usec: c_long,
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
}
pub const E2BIG: c_int = 7;
pub const EACCES: c_int = 13;
pub const EADDRINUSE: c_int = 100;
pub const EADDRNOTAVAIL: c_int = 101;
pub const EAFNOSUPPORT: c_int = 102;
pub const EAGAIN: c_int = 11;
pub const EALREADY: c_int = 103;
pub const EBADF: c_int = 9;
pub const EBADMSG: c_int = 104;
pub const EBUSY: c_int = 16;
pub const ECANCELED: c_int = 105;
pub const ECHILD: c_int = 10;
pub const ECONNABORTED: c_int = 106;
pub const ECONNREFUSED: c_int = 107;
pub const ECONNRESET: c_int = 108;
pub const EDEADLK: c_int = 36;
pub const EDEADLOCK: c_int = 36;
pub const EDESTADDRREQ: c_int = 109;
pub const EDOM: c_int = 33;
pub const EEXIST: c_int = 17;
pub const EFAULT: c_int = 14;
pub const EFBIG: c_int = 27;
pub const EHOSTUNREACH: c_int = 110;
pub const EIDRM: c_int = 111;
pub const EILSEQ: c_int = 42;
pub const EINPROGRESS: c_int = 112;
pub const EINTR: c_int = 4;
pub const EINVAL: c_int = 22;
pub const EIO: c_int = 5;
pub const EISCONN: c_int = 113;
pub const EISDIR: c_int = 21;
pub const ELOOP: c_int = 114;
pub const EMFILE: c_int = 24;
pub const EMLINK: c_int = 31;
pub const EMSGSIZE: c_int = 115;
pub const ENAMETOOLONG: c_int = 38;
pub const ENETDOWN: c_int = 116;
pub const ENETRESET: c_int = 117;
pub const ENETUNREACH: c_int = 118;
pub const ENFILE: c_int = 23;
pub const ENOBUFS: c_int = 119;
pub const ENODATA: c_int = 120;
pub const ENODEV: c_int = 19;
pub const ENOENT: c_int = 2;
pub const ENOEXEC: c_int = 8;
pub const ENOLCK: c_int = 39;
pub const ENOLINK: c_int = 121;
pub const ENOMEM: c_int = 12;
pub const ENOMSG: c_int = 122;
pub const ENOPROTOOPT: c_int = 123;
pub const ENOSPC: c_int = 28;
pub const ENOSR: c_int = 124;
pub const ENOSTR: c_int = 125;
pub const ENOSYS: c_int = 40;
pub const ENOTCONN: c_int = 126;
pub const ENOTDIR: c_int = 20;
pub const ENOTEMPTY: c_int = 41;
pub const ENOTRECOVERABLE: c_int = 127;
pub const ENOTSOCK: c_int = 128;
pub const ENOTSUP: c_int = 129;
pub const ENOTTY: c_int = 25;
pub const ENXIO: c_int = 6;
pub const EOPNOTSUPP: c_int = 130;
pub const EOVERFLOW: c_int = 132;
pub const EOWNERDEAD: c_int = 133;
pub const EPERM: c_int = 1;
pub const EPIPE: c_int = 32;
pub const EPROTO: c_int = 134;
pub const EPROTONOSUPPORT: c_int = 135;
pub const EPROTOTYPE: c_int = 136;
pub const ERANGE: c_int = 34;
pub const EROFS: c_int = 30;
pub const ESPIPE: c_int = 29;
pub const ESRCH: c_int = 3;
pub const ETIME: c_int = 137;
pub const ETIMEDOUT: c_int = 138;
pub const ETXTBSY: c_int = 139;
pub const EWOULDBLOCK: c_int = 140;
pub const EXDEV: c_int = 18;
pub const LC_ALL: c_int = 0;
pub const LC_COLLATE: c_int = 1;
pub const LC_CTYPE: c_int = 2;
pub const LC_MONETARY: c_int = 3;
pub const LC_NUMERIC: c_int = 4;
pub const LC_TIME: c_int = 5;
pub const O_APPEND: c_int = 0x0008;
pub const O_BINARY: c_int = 0x8000;
pub const O_CREAT: c_int = 0x0100;
pub const O_EXCL: c_int = 0x0400;
pub const O_NOINHERIT: c_int = 0x0080;
pub const O_RANDOM: c_int = 0x0010;
pub const O_RDONLY: c_int = 0x0000;
pub const O_RDWR: c_int = 0x0002;
pub const O_SEQUENTIAL: c_int = 0x0020;
pub const O_TEMPORARY: c_int = 0x0040;
pub const O_TEXT: c_int = 0x4000;
pub const O_TRUNC: c_int = 0x0200;
pub const O_WRONLY: c_int = 0x0001;
pub const SEEK_CUR: c_int = 1;
pub const SEEK_END: c_int = 2;
pub const SEEK_SET: c_int = 0;
pub const SIGABRT: c_int = 22;
pub const SIGFPE: c_int = 8;
pub const SIGILL: c_int = 4;
pub const SIGINT: c_int = 2;
pub const SIGSEGV: c_int = 11;
pub const SIGTERM: c_int = 15;
pub const SIG_DFL: sighandler_t = 0;
pub const SIG_ERR: c_int = -1;
pub const SIG_IGN: sighandler_t = 1;
pub const S_IFCHR: c_int = 0o2_0000;
pub const S_IFDIR: c_int = 0o4_0000;
pub const S_IFMT: c_int = 0o17_0000;
pub const S_IFREG: c_int = 0o10_0000;
unsafe extern "C" {
    pub fn _exit(status: c_int) -> !;
    pub fn abort() -> !;
    #[link_name = "_access"]
    pub fn access(path: *const c_char, amode: c_int) -> c_int;
    pub fn atexit(cb: extern "C" fn()) -> c_int;
    #[link_name = "_chdir"]
    pub fn chdir(dir: *const c_char) -> c_int;
    #[link_name = "_close"]
    pub fn close(fd: c_int) -> c_int;
    #[link_name = "_commit"]
    pub fn commit(fd: c_int) -> c_int;
    #[link_name = "_dup"]
    pub fn dup(fd: c_int) -> c_int;
    #[link_name = "_dup2"]
    pub fn dup2(src: c_int, dst: c_int) -> c_int;
    #[link_name = "_execve"]
    pub fn execve(
        prog: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> c_int;
    pub fn exit(status: c_int) -> !;
    pub fn fclose(file: *mut FILE) -> c_int;
    #[link_name = "_fdopen"]
    pub fn fdopen(fd: c_int, mode: *const c_char) -> *mut FILE;
    #[link_name = "_fstat64"]
    pub fn fstat(fildes: c_int, buf: *mut stat) -> c_int;
    #[link_name = "_get_osfhandle"]
    pub fn get_osfhandle(fd: c_int) -> intptr_t;
    #[link_name = "_getpid"]
    pub fn getpid() -> c_int;
    #[link_name = "_isatty"]
    pub fn isatty(fd: c_int) -> c_int;
    #[link_name = "_lseek"]
    pub fn lseek(fd: c_int, offset: c_long, origin: c_int) -> c_long;
    pub fn memmove(dest: *mut c_void, src: *const c_void, n: size_t) -> *mut c_void;
    pub fn memset(dest: *mut c_void, c: c_int, n: size_t) -> *mut c_void;
    #[link_name = "_open"]
    pub fn open(path: *const c_char, oflag: c_int, ...) -> c_int;
    #[link_name = "_open_osfhandle"]
    pub fn open_osfhandle(osfhandle: intptr_t, flags: c_int) -> c_int;
    #[link_name = "_pipe"]
    pub fn pipe(fds: *mut c_int, psize: c_uint, textmode: c_int) -> c_int;
    pub fn raise(signum: c_int) -> c_int;
    #[link_name = "_read"]
    pub fn read(fd: c_int, buf: *mut c_void, count: c_uint) -> c_int;
    pub fn rename(oldname: *const c_char, newname: *const c_char) -> c_int;
    pub fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
    pub fn signal(signum: c_int, handler: sighandler_t) -> sighandler_t;
    #[link_name = "_stat64"]
    pub fn stat(path: *const c_char, buf: *mut stat) -> c_int;
    pub fn strcoll(cs: *const c_char, ct: *const c_char) -> c_int;
    pub fn strerror(n: c_int) -> *mut c_char;
    pub fn strxfrm(s: *mut c_char, ct: *const c_char, n: size_t) -> size_t;
    pub fn system(s: *const c_char) -> c_int;
    #[link_name = "_time64"]
    pub fn time(destTime: *mut time_t) -> time_t;
    #[link_name = "_wopen"]
    pub fn wopen(path: *const wchar_t, oflag: c_int, ...) -> c_int;
    #[link_name = "_write"]
    pub fn write(fd: c_int, buf: *const c_void, count: c_uint) -> c_int;
}
unsafe extern "system" {
    pub fn bind(s: SOCKET, name: *const sockaddr, namelen: c_int) -> c_int;
    pub fn connect(s: SOCKET, name: *const sockaddr, namelen: c_int) -> c_int;
    pub fn getsockopt(
        s: SOCKET,
        level: c_int,
        optname: c_int,
        optval: *mut c_char,
        optlen: *mut c_int,
    ) -> c_int;
    pub fn setsockopt(
        s: SOCKET,
        level: c_int,
        optname: c_int,
        optval: *const c_char,
        optlen: c_int,
    ) -> c_int;
    pub fn socket(af: c_int, socket_type: c_int, protocol: c_int) -> SOCKET;
}
