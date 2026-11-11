//! Operations in flight on the JS side, and their answers (`psh_complete`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use crate::{export, sys, take_input};

/// The JS side's answer to an operation.
#[derive(Debug, Clone, Default)]
pub struct Answer {
    /// 0, or an errno (`ENOENT` = 2, ...).
    pub err: i32,
    pub data: Vec<u8>,
}

impl Answer {
    pub fn result(self) -> io::Result<Answer> {
        if self.err == 0 {
            Ok(self)
        } else {
            Err(errno_error(self.err))
        }
    }
}

/// The errors the JS host reports, as errno numbers (Linux numbering), with the text
/// bash prints for them (`strerror`).
pub fn errno_error(err: i32) -> io::Error {
    use io::ErrorKind::*;
    let (kind, text) = match err {
        1 => (PermissionDenied, "Operation not permitted"),
        2 => (NotFound, "No such file or directory"),
        9 => (Other, "Bad file descriptor"),
        13 => (PermissionDenied, "Permission denied"),
        16 => (ResourceBusy, "Device or resource busy"),
        17 => (AlreadyExists, "File exists"),
        18 => (CrossesDevices, "Invalid cross-device link"),
        20 => (NotADirectory, "Not a directory"),
        21 => (IsADirectory, "Is a directory"),
        22 => (InvalidInput, "Invalid argument"),
        27 => (FileTooLarge, "File too large"),
        28 => (StorageFull, "No space left on device"),
        30 => (ReadOnlyFilesystem, "Read-only file system"),
        32 => (BrokenPipe, "Broken pipe"),
        36 => (InvalidFilename, "File name too long"),
        39 => (DirectoryNotEmpty, "Directory not empty"),
        40 => (Other, "Too many levels of symbolic links"),
        95 => (Unsupported, "Operation not supported"),
        _ => (Other, "Input/output error"),
    };
    io::Error::new(kind, ErrorText(text))
}

/// The text of an error from the JS host (a small type: a `&str` message would pull in
/// `String`'s error machinery).
#[derive(Debug)]
struct ErrorText(&'static str);

impl std::fmt::Display for ErrorText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ErrorText {}

enum Slot {
    Waiting(Option<Waker>),
    Done(Answer),
}

thread_local! {
    static NEXT: Cell<u32> = const { Cell::new(1) };
    static OPS: RefCell<HashMap<u32, Slot>> = RefCell::new(HashMap::new());
}

/// Starts an operation the JS side answers later: registers it, then asks.
pub fn start(kind: u32, arg: u32, data: &[u8]) -> u32 {
    let op = next();
    wait(op);
    sys::call(kind, op, arg, data);
    op
}

/// A fresh operation id.
pub fn next() -> u32 {
    NEXT.with(|n| {
        let op = n.get();
        n.set(op.wrapping_add(1).max(1));
        op
    })
}

/// Registers an operation whose answer comes later (`psh_complete`).
pub fn wait(op: u32) {
    OPS.with(|o| o.borrow_mut().insert(op, Slot::Waiting(None)));
}

/// The answer, if it came; else remembers the waker.
pub fn poll(op: u32, cx: &mut Context<'_>) -> Poll<Answer> {
    OPS.with(|o| {
        let mut ops = o.borrow_mut();
        match ops.remove(&op) {
            Some(Slot::Done(a)) => Poll::Ready(a),
            Some(Slot::Waiting(_)) | None => {
                ops.insert(op, Slot::Waiting(Some(cx.waker().clone())));
                Poll::Pending
            }
        }
    })
}

/// Forgets an operation whose result nobody wants any more; a late answer is ignored.
pub fn forget(op: u32) {
    OPS.with(|o| o.borrow_mut().remove(&op));
}

fn complete(op: u32, answer: Answer) {
    let waker = OPS.with(|o| {
        let mut ops = o.borrow_mut();
        match ops.get_mut(&op) {
            Some(slot) => match std::mem::replace(slot, Slot::Done(answer)) {
                Slot::Waiting(w) => w,
                Slot::Done(_) => None,
            },
            None => None,
        }
    });
    // Woken outside the borrow: the waker may call back into the module.
    if let Some(w) = waker {
        w.wake();
    }
}

export! {
    /// The answer to operation `op`: an errno (0 for success) and `len` bytes of data
    /// in the input buffer.
    fn psh_complete(op: u32, err: i32, len: usize) {
        let data = take_input(len);
        complete(op, Answer { err, data });
    }
}

/// A future over one operation.
pub struct OpFuture {
    op: u32,
    done: bool,
}

impl OpFuture {
    pub fn new(op: u32) -> OpFuture {
        OpFuture { op, done: false }
    }
}

impl Future for OpFuture {
    type Output = Answer;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Answer> {
        let r = poll(self.op, cx);
        if r.is_ready() {
            self.done = true;
        }
        r
    }
}

impl Drop for OpFuture {
    fn drop(&mut self) {
        if !self.done {
            forget(self.op);
        }
    }
}
