//! The module's one import, `psh.host_call`, and its native stand-in for tests.
//!
//! Every request to the JS side goes through `host_call(kind, op, arg, ptr, len)`: the
//! kind of request (the `K_*` constants), an operation id the JS side answers with
//! `psh_complete` (0 for requests without an answer), one integer argument, and a byte
//! payload in the module's memory. The JS side may answer an operation during the call
//! itself (an in-memory file system usually does) or later from its event loop.

// The parser module uses only K_PANIC.
#![cfg_attr(not(feature = "shell"), allow(dead_code))]

/// Opens a file: arg = open flags (`F_*`; with `F_MODE`, the permission bits of a file
/// it creates in bits 16..28), payload = path.
pub const K_OPEN: u32 = 1;
/// stat/lstat: arg = 1 to follow symlinks, payload = path.
pub const K_STAT: u32 = 2;
/// access: arg = 0 read, 1 write, 2 execute; payload = path.
pub const K_ACCESS: u32 = 3;
/// Directory entries: payload = path.
pub const K_READ_DIR: u32 = 4;
/// The physical path: payload = path.
pub const K_CANONICALIZE: u32 = 5;
/// Command lookup: payload = name, PATH and cwd (length-prefixed).
pub const K_RESOLVE: u32 = 6;
/// Starts a host command: payload = the spawn spec (see `host.rs`). Answered during
/// the call.
pub const K_SPAWN: u32 = 7;
/// A timer: arg = milliseconds.
pub const K_SLEEP: u32 = 8;
/// Reads from a handle: arg = handle, len = the most bytes wanted (no payload).
pub const K_READ: u32 = 9;
/// Writes to a handle: arg = handle, payload = the bytes.
pub const K_WRITE: u32 = 10;
/// Closes a handle: arg = handle.
pub const K_CLOSE: u32 = 11;
/// Waits for a command: arg = process id.
pub const K_WAIT: u32 = 12;
/// Sends a signal: arg = process id, len = the signal.
pub const K_SIGNAL: u32 = 13;
/// Lets a command run on unobserved: arg = process id.
pub const K_RELEASE: u32 = 14;
/// Abandons a command (TERM, then KILL): arg = process id.
pub const K_ABANDON: u32 = 15;
/// A blocked shell has work again: arg = shell id.
pub const K_WAKE: u32 = 16;
/// A panic message (the module aborts after it): payload = the message.
pub const K_PANIC: u32 = 17;
/// The shell's `$$`: returned by the call.
pub const K_PID: u32 = 18;
/// The time since the Unix epoch: the JS side puts it into the input buffer (`psh_in`)
/// as a little-endian f64 of milliseconds and returns its length, 8.
pub const K_NOW: u32 = 19;
/// A file system change or `stat_extra` (D1150): arg = the `FS_*` operation, payload = its
/// fields (length-prefixed paths, u32s; times as decimal text). `FS_STAT_EXTRA` answers
/// `stat`'s text plus `atime_ns ctime_ns btime_ns|- nlink blocks blksize rdev`;
/// `FS_READ_LINK` the target.
pub const K_FS: u32 = 20;
/// A user (op 0) or group (op 1) name: arg = the id. The JS side puts the name into the
/// input buffer and returns its length, or -1 when it has none.
pub const K_NAME: u32 = 21;
/// The local time's offset east of UTC in seconds, returned by the call: payload = the
/// instant, epoch seconds as decimal text.
pub const K_UTC_OFFSET: u32 = 22;

/// `K_FS` operations.
pub const FS_STAT_EXTRA: u32 = 0;
pub const FS_MKDIR: u32 = 1;
pub const FS_REMOVE_FILE: u32 = 2;
pub const FS_REMOVE_DIR: u32 = 3;
pub const FS_RENAME: u32 = 4;
pub const FS_SYMLINK: u32 = 5;
pub const FS_HARD_LINK: u32 = 6;
pub const FS_READ_LINK: u32 = 7;
pub const FS_SET_MODE: u32 = 8;
pub const FS_SET_TIMES: u32 = 9;

#[cfg(target_arch = "wasm32")]
mod imp {
    // The import itself: declared here, provided by js/psh.mjs.
    #[allow(unsafe_code)]
    #[link(wasm_import_module = "psh")]
    unsafe extern "C" {
        fn host_call(kind: u32, op: u32, arg: u32, ptr: *const u8, len: usize) -> i32;
    }

    pub fn call(kind: u32, op: u32, arg: u32, data: &[u8]) -> i32 {
        call_len(kind, op, arg, data.as_ptr(), data.len())
    }

    #[allow(dead_code)]
    pub fn call_len(kind: u32, op: u32, arg: u32, ptr: *const u8, len: usize) -> i32 {
        // SAFETY: `host_call` is the module's own import; the JS glue (js/psh.mjs) only
        // reads `len` bytes at `ptr`, which point into a live slice (or are a length
        // with a dangling but unused pointer for K_READ/K_SIGNAL), and never retains
        // the pointer past the call.
        #[allow(unsafe_code)]
        unsafe {
            host_call(kind, op, arg, ptr, len)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    //! Native builds have no JS side: tests install a Rust function in its place.
    use std::cell::RefCell;

    type Handler = Box<dyn FnMut(u32, u32, u32, &[u8], usize) -> i32>;

    thread_local! {
        static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    }

    /// Installs the stand-in for the JS side (tests).
    #[cfg(test)]
    pub fn set_handler(h: impl FnMut(u32, u32, u32, &[u8], usize) -> i32 + 'static) {
        HANDLER.with(|c| *c.borrow_mut() = Some(Box::new(h)));
    }

    pub fn call(kind: u32, op: u32, arg: u32, data: &[u8]) -> i32 {
        dispatch(kind, op, arg, data, data.len())
    }

    #[allow(dead_code)]
    pub fn call_len(kind: u32, op: u32, arg: u32, _ptr: *const u8, len: usize) -> i32 {
        dispatch(kind, op, arg, &[], len)
    }

    fn dispatch(kind: u32, op: u32, arg: u32, data: &[u8], len: usize) -> i32 {
        // Taken out while it runs: the handler may call back into the module.
        let h = HANDLER.with(|c| c.borrow_mut().take());
        let Some(mut h) = h else {
            panic!("no JS side in a native build (host_call kind {kind})");
        };
        let r = h(kind, op, arg, data, len);
        HANDLER.with(|c| {
            let mut slot = c.borrow_mut();
            if slot.is_none() {
                *slot = Some(h);
            }
        });
        r
    }
}

pub use imp::call;
#[cfg(feature = "shell")]
pub use imp::call_len;
#[cfg(test)]
#[allow(unused_imports)]
pub use imp::set_handler;
