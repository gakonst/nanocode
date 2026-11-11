//! nanocode-shell's host over the JS side: its file system, its commands and its
//! machine. A file system operation is one `host_call` (`K_FS`) that the JS side
//! answers with `psh_complete`, during the call or later; open files, pipes and the
//! captured output live on this side (nanocode-shell), so reading a file crosses the
//! boundary once, not once per chunk.
//!
//! Payloads are length-prefixed where they carry several values: a `u32` little-endian
//! length, then the bytes. `stat` answers 24 bytes (size and mtime in milliseconds as
//! f64, mode and kind as u32); `list` answers NUL-separated names.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Poll, Waker};
use std::time::Duration;

use bytes::Bytes;
use nanocode_shell::{Call, Commands, FileSystem, Kind, Machine, Remove, Reply, Stat, Write};

use crate::ops::{self, Answer, OpFuture};
use crate::{export, sys, take_input};

pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn new() -> Writer {
        Writer(Vec::new())
    }

    pub fn u32(&mut self, v: u32) -> &mut Writer {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Writer {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
        self
    }

    pub fn raw(&mut self, b: &[u8]) -> &mut Writer {
        self.0.extend_from_slice(b);
        self
    }
}

/// Reads the length-prefixed fields of a payload.
pub struct Reader<'a> {
    pub data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn u32(&mut self) -> Option<u32> {
        let (head, rest) = self.data.split_at_checked(4)?;
        self.data = rest;
        Some(u32::from_le_bytes(head.try_into().ok()?))
    }

    pub fn f64(&mut self) -> Option<f64> {
        let (head, rest) = self.data.split_at_checked(8)?;
        self.data = rest;
        Some(f64::from_le_bytes(head.try_into().ok()?))
    }

    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        let (head, rest) = self.data.split_at_checked(n)?;
        self.data = rest;
        Some(head)
    }
}

/// One file system operation: answered by the call itself (its return value is the
/// length of the answer in the input buffer, or a negative errno: the reply is there at
/// once and no operation is kept), or later (`sys::LATER`: a future over the operation).
fn fs<T: 'static>(
    fs_op: u32,
    payload: &[u8],
    read: impl FnOnce(Answer) -> io::Result<T> + 'static,
) -> Reply<T> {
    let id = ops::next();
    match sys::call(sys::K_FS, id, fs_op, payload) {
        sys::LATER => {
            ops::wait(id);
            let fut = OpFuture::new(id);
            Reply::Later(Box::pin(async move { fut.await.result().and_then(read) }))
        }
        err if err < 0 => Reply::Now(Err(ops::errno_error(-err))),
        len => Reply::Now(read(Answer {
            err: 0,
            data: take_input(len as usize),
        })),
    }
}

fn unit(_: Answer) -> io::Result<()> {
    Ok(())
}

fn bad_answer() -> io::Error {
    io::Error::other("bad answer from the JS host")
}

/// The JS host's file system.
pub struct JsFs;

impl FileSystem for JsFs {
    fn stat(&self, path: &[u8], follow: bool) -> Reply<Stat> {
        let fs_op = if follow { sys::FS_STAT } else { sys::FS_LSTAT };
        fs(fs_op, path, |a| {
            let mut r = Reader { data: &a.data };
            let (size, mtime_ms) = (
                r.f64().ok_or_else(bad_answer)?,
                r.f64().ok_or_else(bad_answer)?,
            );
            let (mode, kind) = (
                r.u32().ok_or_else(bad_answer)?,
                r.u32().ok_or_else(bad_answer)?,
            );
            Ok(Stat {
                kind: match kind {
                    0 => Kind::File,
                    1 => Kind::Dir,
                    2 => Kind::Symlink,
                    _ => return Err(bad_answer()),
                },
                size: size as u64,
                mode,
                mtime_ns: (mtime_ms * 1e6) as i64,
            })
        })
    }

    fn read(&self, path: &[u8], _limit: u64) -> Reply<Bytes> {
        // The JS side knows the limit (it configured it) and refuses with EFBIG;
        // nanocode-shell checks the length of what comes back.
        fs(sys::FS_READ, path, |a| Ok(Bytes::from(a.data)))
    }

    fn write(&self, path: &[u8], data: &[u8], how: Write) -> Reply<()> {
        let fs_op = match how {
            Write::Replace => sys::FS_WRITE_REPLACE,
            Write::Append => sys::FS_WRITE_APPEND,
            Write::New => sys::FS_WRITE_NEW,
        };
        let mut w = Writer(Vec::with_capacity(4 + path.len() + data.len()));
        w.bytes(path).raw(data);
        fs(fs_op, &w.0, unit)
    }

    fn list(&self, path: &[u8]) -> Reply<Vec<Vec<u8>>> {
        fs(sys::FS_LIST, path, |a| {
            Ok(a.data
                .split(|&b| b == 0)
                .filter(|n| !n.is_empty())
                .map(<[u8]>::to_vec)
                .collect())
        })
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> Reply<()> {
        let mut w = Writer::new();
        w.u32(mode).raw(path);
        fs(sys::FS_MKDIR, &w.0, unit)
    }

    fn remove(&self, path: &[u8], what: Remove) -> Reply<()> {
        let fs_op = match what {
            Remove::File => sys::FS_REMOVE_FILE,
            Remove::Dir => sys::FS_REMOVE_DIR,
        };
        fs(fs_op, path, unit)
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> Reply<()> {
        let mut w = Writer::new();
        w.bytes(from).raw(to);
        fs(sys::FS_RENAME, &w.0, unit)
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> Reply<()> {
        let mut w = Writer::new();
        w.bytes(target).raw(link);
        fs(sys::FS_SYMLINK, &w.0, unit)
    }

    fn read_link(&self, path: &[u8]) -> Reply<Vec<u8>> {
        fs(sys::FS_READ_LINK, path, |a| Ok(a.data))
    }

    fn set_mode(&self, path: &[u8], mode: u32) -> Reply<()> {
        let mut w = Writer::new();
        w.u32(mode).raw(path);
        fs(sys::FS_SET_MODE, &w.0, unit)
    }

    fn set_mtime(&self, path: &[u8], mtime_ns: i64) -> Reply<()> {
        let mut w = Writer::new();
        w.raw(&(mtime_ns as f64 / 1e6).to_le_bytes()).raw(path);
        fs(sys::FS_SET_MTIME, &w.0, unit)
    }
}

/// The JS host's clock, timers and entropy.
pub struct JsMachine;

impl Machine for JsMachine {
    fn now(&self) -> Duration {
        let n = sys::call(sys::K_NOW, 0, 0, &[]);
        let data = take_input(n.max(0) as usize);
        let ms = data
            .get(..8)
            .and_then(|b| b.try_into().ok())
            .map(f64::from_le_bytes)
            .unwrap_or(0.0);
        if ms.is_finite() && ms > 0.0 {
            Duration::from_secs_f64(ms / 1000.0)
        } else {
            Duration::ZERO
        }
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()>>> {
        let ms = duration.as_millis().min(u32::MAX as u128) as u32;
        let fut = OpFuture::new(ops::start(sys::K_SLEEP, ms, &[]));
        Box::pin(async move {
            fut.await;
        })
    }

    fn random(&self, bytes: &mut [u8]) {
        let n = sys::call_len(sys::K_RANDOM, 0, 0, std::ptr::null(), bytes.len());
        let data = take_input(n.max(0) as usize);
        let n = data.len().min(bytes.len());
        bytes[..n].copy_from_slice(&data[..n]);
    }

    fn utc_offset(&self, epoch_secs: i64) -> i32 {
        sys::call(sys::K_UTC_OFFSET, 0, 0, epoch_secs.to_string().as_bytes())
    }
}

/// What a running host command asked for and has not been given yet.
enum Request {
    Read { req: u32, max: usize },
    Write { req: u32, fd: u32, data: Vec<u8> },
}

#[derive(Default)]
struct Proc {
    requests: VecDeque<Request>,
    exit: Option<i32>,
    waker: Option<Waker>,
}

thread_local! {
    static PROCS: RefCell<(u32, HashMap<u32, Proc>)> = RefCell::new((0, HashMap::new()));
}

/// Queues what a command asked for and wakes the task that serves it.
fn ask(pid: u32, change: impl FnOnce(&mut Proc)) {
    let waker = PROCS.with(|p| {
        let mut procs = p.borrow_mut();
        let proc = procs.1.get_mut(&pid)?;
        change(proc);
        proc.waker.take()
    });
    // Woken outside the borrow: the waker may call back into the module.
    if let Some(w) = waker {
        w.wake();
    }
}

export! {
    /// A host command reads its stdin: at most `max` bytes, answered with `K_DONE`
    /// whose argument has `READ_END` set when the input ended with these bytes (no
    /// bytes: end of input).
    fn psh_proc_read(pid: u32, req: u32, max: usize) {
        ask(pid, |p| p.requests.push_back(Request::Read { req, max }));
    }
}

export! {
    /// A host command writes the `len` input bytes to its stdout (`fd` 1) or stderr
    /// (`fd` 2), answered with `K_DONE` once they are all written. With `req` 0 no
    /// answer is sent (a command's last output, followed at once by its exit): if
    /// nobody reads it any more the command counts as ended by SIGPIPE (141).
    fn psh_proc_write(pid: u32, req: u32, fd: u32, len: usize) {
        let data = take_input(len);
        ask(pid, |p| p.requests.push_back(Request::Write { req, fd, data }));
    }
}

export! {
    /// A host command ended with this status.
    fn psh_proc_exit(pid: u32, status: i32) {
        ask(pid, |p| p.exit = Some(status));
    }
}

/// Removes the command from the table when its task ends or is dropped (a signal, a
/// cancelled run), and tells the JS side if it was still running.
struct Running(u32);

impl Drop for Running {
    fn drop(&mut self) {
        let proc = PROCS.with(|p| p.borrow_mut().1.remove(&self.0));
        if proc.is_some_and(|p| p.exit.is_none()) {
            sys::call_len(sys::K_KILL, 0, self.0, std::ptr::null(), 15);
        }
    }
}

fn errno(e: &io::Error) -> u32 {
    use io::ErrorKind::*;
    match e.kind() {
        NotFound => 2,
        PermissionDenied => 13,
        AlreadyExists => 17,
        BrokenPipe => 32,
        _ => 5,
    }
}

/// The JS host's commands.
pub struct JsCommands;

impl Commands for JsCommands {
    fn has(&self, name: &[u8]) -> bool {
        sys::call(sys::K_HAS, 0, 0, name) != 0
    }

    fn run(&self, call: Call) -> Pin<Box<dyn Future<Output = i32>>> {
        let pid = PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            procs.0 = procs.0.wrapping_add(1).max(1);
            let pid = procs.0;
            procs.1.insert(pid, Proc::default());
            pid
        });
        let mut w = Writer::new();
        w.u32(call.argv.len() as u32);
        for a in &call.argv {
            w.bytes(a);
        }
        w.u32(call.env.len() as u32);
        for (k, v) in &call.env {
            w.bytes(k).bytes(v);
        }
        w.bytes(&call.cwd);
        // Bit 0: stdout is the shell's captured output, bit 1: stderr is, bit 2: stdin is
        // the input of a machine the shell stands in for, which it has no answer for.
        w.u32(
            u32::from(call.stdout.captured())
                | u32::from(call.stderr.captured()) << 1
                | u32::from(call.stdin.unanswered()) << 2,
        );
        Box::pin(async move {
            let running = Running(pid);
            // A last write that found its pipe closed (`psh_proc_write` without an answer).
            let mut piped = false;
            sys::call(sys::K_SPAWN, 0, pid, &w.0);
            loop {
                let next = std::future::poll_fn(|cx| {
                    PROCS.with(|p| {
                        let mut procs = p.borrow_mut();
                        let Some(proc) = procs.1.get_mut(&pid) else {
                            return Poll::Ready(Err(1));
                        };
                        if let Some(request) = proc.requests.pop_front() {
                            return Poll::Ready(Ok(request));
                        }
                        if let Some(status) = proc.exit {
                            return Poll::Ready(Err(status));
                        }
                        proc.waker = Some(cx.waker().clone());
                        Poll::Pending
                    })
                })
                .await;
                match next {
                    Ok(Request::Read { req, max }) => {
                        let mut buf = vec![0; max];
                        match call.stdin.read(&mut buf).await {
                            Ok(mut n) => {
                                // What is there already comes along, and so does the
                                // end of the input: one answer instead of two.
                                let mut end = n == 0;
                                while !end && n < max {
                                    match call.stdin.read_ready(&mut buf[n..]) {
                                        Some(Ok(0)) => end = true,
                                        Some(Ok(more)) => n += more,
                                        Some(Err(_)) | None => break,
                                    }
                                }
                                let arg = if end { sys::READ_END } else { 0 };
                                sys::call(sys::K_DONE, req, arg, &buf[..n])
                            }
                            Err(e) => sys::call(sys::K_DONE, req, errno(&e), &[]),
                        };
                    }
                    Ok(Request::Write { req, fd, data }) => {
                        let stream = if fd == 2 { &call.stderr } else { &call.stdout };
                        let wrote = stream.write_all(&data).await;
                        if req == 0 {
                            piped |=
                                matches!(&wrote, Err(e) if e.kind() == io::ErrorKind::BrokenPipe);
                            continue;
                        }
                        match wrote {
                            Ok(()) => sys::call(sys::K_DONE, req, 0, &[]),
                            Err(e) => sys::call(sys::K_DONE, req, errno(&e), &[]),
                        };
                    }
                    Err(status) => {
                        drop(running);
                        return if piped { 128 + 13 } else { status };
                    }
                }
            }
        })
    }
}
