#[allow(unused_imports)]
use super::*;
unsafe extern "C" {
    #[link_name = "close$NOCANCEL"]
    pub fn close(fd: c_int) -> c_int;
    #[link_name = "fstat$INODE64"]
    pub fn fstat(fildes: c_int, buf: *mut stat) -> c_int;
    #[link_name = "select$1050"]
    pub fn select(
        nfds: c_int,
        readfds: *mut fd_set,
        writefds: *mut fd_set,
        errorfds: *mut fd_set,
        timeout: *mut timeval,
    ) -> c_int;
    #[link_name = "stat$INODE64"]
    pub fn stat(path: *const c_char, buf: *mut stat) -> c_int;
}
