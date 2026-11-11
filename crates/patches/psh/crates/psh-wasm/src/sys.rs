//! The module's one import, `psh.host_call`, and its native stand-in for tests.
//!
//! Every request to the JS side goes through `host_call(kind, op, arg, ptr, len)`: the
//! kind of request (the `K_*` constants), an operation id the JS side answers with
//! `psh_complete` (0 for requests without an answer), one integer argument, and a byte
//! payload in the module's memory. The JS side may answer an operation during the call
//! itself (a file system that answers synchronously does) or later from its event loop.

// The parser module uses only K_PANIC.
#![cfg_attr(not(feature = "shell"), allow(dead_code))]

/// A file system operation: arg = the `FS_*` operation, payload = its fields (see
/// `host.rs`). The call returns [`LATER`] when the JS side will answer with
/// `psh_complete`, a negative errno when it failed at once, else the length of the
/// answer it put into the input buffer.
pub const K_FS: u32 = 1;
/// `K_FS`'s return value for an answer that comes later.
pub const LATER: i32 = i32::MIN;
/// A timer: arg = milliseconds. Answered with `psh_complete`.
pub const K_SLEEP: u32 = 2;
/// The time since the Unix epoch: the JS side puts it into the input buffer (`psh_in`)
/// as a little-endian f64 of milliseconds and returns its length, 8.
pub const K_NOW: u32 = 3;
/// Random bytes: len = how many (no payload). The JS side puts them into the input
/// buffer and returns their count.
pub const K_RANDOM: u32 = 4;
/// Whether the host has a command: payload = its name. Returns 1 or 0.
pub const K_HAS: u32 = 5;
/// Starts a host command: arg = its process id, payload = the call (the arguments and
/// the environment as counted lists, the working directory, and a u32 whose bits 0 and
/// 1 say that stdout and stderr are the shell's captured output, and bit 2 that stdin
/// is the input of a machine the shell stands in for, which it has no answer for).
/// The command then asks for its I/O with `psh_proc_read` and `psh_proc_write` and
/// ends with `psh_proc_exit`.
pub const K_SPAWN: u32 = 6;
/// A host command was signalled or abandoned: arg = process id, len = the signal.
pub const K_KILL: u32 = 7;
/// The answer to a command's read or write: op = the request id the JS side chose,
/// arg = an errno (0 for success; a read may have [`READ_END`] instead), payload = the
/// bytes read.
pub const K_DONE: u32 = 8;
/// `K_DONE`'s argument for a read whose bytes are the input's last.
pub const READ_END: u32 = 1 << 16;
/// A blocked shell has work again: arg = shell id.
pub const K_WAKE: u32 = 9;
/// A panic message (the module aborts after it): payload = the message.
pub const K_PANIC: u32 = 10;
/// The local time's offset east of UTC in seconds, returned by the call: payload = the
/// instant, epoch seconds as decimal text.
pub const K_UTC_OFFSET: u32 = 11;

/// An HTTP request for curl and wget (nanocode's addition), where the JS side gave the
/// shell a network (`psh_shell_fetch`): payload = the method, the URL, the headers (a
/// count, then name and value each), the body (empty for none), how many redirects to
/// follow as a u32 ([`NO_FOLLOW`]: none, a 3xx is the answer), the timeout in
/// milliseconds as an f64 (0 for none) and the most body bytes to keep as an f64.
/// Answered with `psh_complete`: error 0 and the response (the protocol, the status as
/// a u32, the reason, the headers as they came, the body, the final URL and how many
/// redirects were followed as a u32), or one of the `FETCH_*` failures with its detail
/// as text.
pub const K_FETCH: u32 = 12;
/// `K_FETCH`'s redirect count for a request whose redirects are not followed.
pub const NO_FOLLOW: u32 = u32::MAX;
/// `K_FETCH` failures: the name did not resolve, nothing answered at the address, the
/// timeout passed, more redirects than asked for, TLS, a body past the limit, the
/// host's policy, anything else.
pub const FETCH_RESOLVE: i32 = 1;
pub const FETCH_CONNECT: i32 = 2;
pub const FETCH_TIMEOUT: i32 = 3;
pub const FETCH_REDIRECTS: i32 = 4;
pub const FETCH_SECURE: i32 = 5;
pub const FETCH_TOO_LARGE: i32 = 6;
pub const FETCH_DENIED: i32 = 7;

/// `K_FS` operations.
pub const FS_STAT: u32 = 0;
pub const FS_LSTAT: u32 = 1;
pub const FS_READ: u32 = 2;
pub const FS_WRITE_REPLACE: u32 = 3;
pub const FS_WRITE_APPEND: u32 = 4;
pub const FS_WRITE_NEW: u32 = 5;
pub const FS_LIST: u32 = 6;
pub const FS_MKDIR: u32 = 7;
pub const FS_REMOVE_FILE: u32 = 8;
pub const FS_REMOVE_DIR: u32 = 9;
pub const FS_RENAME: u32 = 10;
pub const FS_SYMLINK: u32 = 11;
pub const FS_READ_LINK: u32 = 12;
pub const FS_SET_MODE: u32 = 13;
pub const FS_SET_MTIME: u32 = 14;

#[cfg(target_arch = "wasm32")]
mod imp {
    // The import itself: declared here, provided by the JS glue.
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
        // SAFETY: `host_call` is the module's own import; the JS glue only reads `len`
        // bytes at `ptr`, which point into a live slice (or are a length with a dangling
        // but unused pointer for K_RANDOM/K_KILL), and never retains the pointer past
        // the call.
        #[allow(unsafe_code)]
        unsafe {
            host_call(kind, op, arg, ptr, len)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    //! Native builds have no JS side: every request panics.

    pub fn call(kind: u32, _op: u32, _arg: u32, _data: &[u8]) -> i32 {
        panic!("no JS side in a native build (host_call kind {kind})");
    }

    #[allow(dead_code)]
    pub fn call_len(kind: u32, _op: u32, _arg: u32, _ptr: *const u8, _len: usize) -> i32 {
        panic!("no JS side in a native build (host_call kind {kind})");
    }
}

pub use imp::call;
#[cfg(feature = "shell")]
pub use imp::call_len;
