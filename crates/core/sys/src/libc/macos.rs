#[allow(unused_imports)]
use super::*;
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct proc_fdinfo {
    pub proc_fd: i32,
    pub proc_fdtype: u32,
}
impl ::core::default::Default for proc_fdinfo {
    #[allow(unused_doc_comments)]
    fn default() -> Self {
        Self {
            proc_fd: ::core::default::Default::default(),
            proc_fdtype: ::core::default::Default::default(),
        }
    }
}
pub const PROC_PIDLISTFDS: c_int = 1;
unsafe extern "C" {
    pub fn clock_settime(clock_id: clockid_t, tp: *const timespec) -> c_int;
    pub fn posix_spawn_file_actions_addchdir_np(
        actions: *mut posix_spawn_file_actions_t,
        path: *const c_char,
    ) -> c_int;
    pub fn proc_listallpids(buffer: *mut c_void, buffersize: c_int) -> c_int;
    pub fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    pub fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    pub fn system(s: *const c_char) -> c_int;
}
