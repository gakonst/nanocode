#[allow(unused_imports)]
use super::*;
pub type blksize_t = i64;
pub type nlink_t = u64;
pub type wchar_t = i32;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
#[repr(packed)]
pub struct epoll_event {
    pub events: u32,
    pub u64: u64,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct pthread_attr_t {
    pub(super) __size: [u64; 7],
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct stat {
    pub st_dev: dev_t,
    pub st_ino: ino_t,
    pub st_nlink: nlink_t,
    pub st_mode: mode_t,
    pub st_uid: uid_t,
    pub st_gid: gid_t,
    pub(super) __pad0: Padding<c_int>,
    pub st_rdev: dev_t,
    pub st_size: off_t,
    pub st_blksize: blksize_t,
    pub st_blocks: blkcnt_t,
    pub st_atime: time_t,
    pub st_atime_nsec: i64,
    pub st_mtime: time_t,
    pub st_mtime_nsec: i64,
    pub st_ctime: time_t,
    pub st_ctime_nsec: i64,
    pub(super) __unused: Padding<[i64; 3]>,
}
pub const O_DIRECT: c_int = 0x4000;
pub const O_DIRECTORY: c_int = 0x10000;
pub const O_NOFOLLOW: c_int = 0x20000;
pub const SYS_accept: c_long = 43;
pub const SYS_accept4: c_long = 288;
pub const SYS_listen: c_long = 50;
pub const SYS_seccomp: c_long = 317;
pub const SYS_sendmmsg: c_long = 307;
pub const SYS_sendmsg: c_long = 46;
pub const SYS_sendto: c_long = 44;
pub const SYS_setpgid: c_long = 109;
pub const SYS_setsid: c_long = 112;
pub const SYS_socket: c_long = 41;
