#[allow(unused_imports)]
use super::*;
pub type blksize_t = c_int;
pub type nlink_t = u32;
pub type wchar_t = u32;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct epoll_event {
    pub events: u32,
    pub u64: u64,
}
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct stat {
    pub st_dev: dev_t,
    pub st_ino: ino_t,
    pub st_mode: mode_t,
    pub st_nlink: nlink_t,
    pub st_uid: uid_t,
    pub st_gid: gid_t,
    pub st_rdev: dev_t,
    pub(super) __pad0: Padding<c_ulong>,
    pub st_size: off_t,
    pub st_blksize: blksize_t,
    pub(super) __pad1: Padding<c_int>,
    pub st_blocks: blkcnt_t,
    pub st_atime: time_t,
    pub st_atime_nsec: c_long,
    pub st_mtime: time_t,
    pub st_mtime_nsec: c_long,
    pub st_ctime: time_t,
    pub st_ctime_nsec: c_long,
    pub(super) __unused: Padding<[c_uint; 2]>,
}
pub const O_DIRECT: c_int = 0x10000;
pub const O_DIRECTORY: c_int = 0x4000;
pub const O_LARGEFILE: c_int = 0x20000;
pub const O_NOFOLLOW: c_int = 0x8000;
pub const SYS_accept: c_long = 202;
pub const SYS_accept4: c_long = 242;
pub const SYS_listen: c_long = 201;
pub const SYS_seccomp: c_long = 277;
pub const SYS_sendmmsg: c_long = 269;
pub const SYS_sendmsg: c_long = 211;
pub const SYS_sendto: c_long = 206;
pub const SYS_setpgid: c_long = 154;
pub const SYS_setsid: c_long = 157;
pub const SYS_socket: c_long = 198;
