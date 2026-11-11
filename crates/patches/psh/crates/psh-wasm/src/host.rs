//! The engine's [`Host`] over the JS side: every operation becomes a `host_call`, every
//! answer a `psh_complete`. Files, pipes and commands live on the JS side; this side
//! holds their handles and process ids.
//!
//! Payloads are length-prefixed where they carry several values: a `u32` little-endian
//! length, then the bytes. Answers that carry several values are text (`stat`) or
//! NUL-separated (`read_dir`).

use std::cell::{Cell, RefCell};
use std::io;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

use psh_engine::host::{
    Access, ChildFd, DirEntry, ExitStatus, ExtProc, ExtraMetadata, FileKind, HostFuture, Metadata,
    OpenOptions, Signal, SpawnSpec, Spawned,
};
use psh_engine::{Host, HostFile};

use crate::ops::{self, OpFuture};
use crate::sys;

/// Flags of `K_OPEN`.
pub const F_READ: u32 = 1;
pub const F_WRITE: u32 = 2;
pub const F_APPEND: u32 = 4;
pub const F_CREATE: u32 = 8;
pub const F_TRUNCATE: u32 = 16;
pub const F_CREATE_NEW: u32 = 32;
/// `OpenOptions::mode` is given, in bits 16..28 of the flags.
pub const F_MODE: u32 = 64;

/// Child descriptor kinds in a spawn spec.
const FD_FILE: u32 = 0;
const FD_PIPE_IN: u32 = 1;
const FD_PIPE_OUT: u32 = 2;
const FD_DUP: u32 = 3;
const FD_NULL: u32 = 4;

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
}

/// The JS host.
pub struct JsHost;

fn op(kind: u32, arg: u32, data: &[u8]) -> OpFuture {
    OpFuture::new(ops::start(kind, arg, data))
}

/// A `K_FS` operation that answers nothing but success or an errno.
fn fs_unit(fs_op: u32, w: &Writer) -> HostFuture<io::Result<()>> {
    let fut = op(sys::K_FS, fs_op, &w.0);
    Box::pin(async move {
        fut.await.result()?;
        Ok(())
    })
}

fn paths(a: &[u8], b: &[u8]) -> Writer {
    let mut w = Writer::new();
    w.bytes(a).bytes(b);
    w
}

fn path_mode(p: &[u8], mode: u32) -> Writer {
    let mut w = Writer::new();
    w.bytes(p).u32(mode);
    w
}

fn name_of(op: u32, id: u32) -> Option<Vec<u8>> {
    let n = sys::call(sys::K_NAME, op, id, &[]);
    (n >= 0).then(|| crate::take_input(n as usize))
}

impl Host for JsHost {
    fn open(&self, path: &[u8], opts: OpenOptions) -> HostFuture<io::Result<Rc<dyn HostFile>>> {
        let mut flags = 0;
        for (on, f) in [
            (opts.read, F_READ),
            (opts.write, F_WRITE),
            (opts.append, F_APPEND),
            (opts.create, F_CREATE),
            (opts.truncate, F_TRUNCATE),
            (opts.create_new, F_CREATE_NEW),
        ] {
            if on {
                flags |= f;
            }
        }
        if let Some(mode) = opts.mode {
            flags |= F_MODE | (mode & 0o7777) << 16;
        }
        let fut = op(sys::K_OPEN, flags, path);
        Box::pin(async move {
            let a = fut.await.result()?;
            Ok(Rc::new(JsFile::new(a.value as u32)) as Rc<dyn HostFile>)
        })
    }

    fn stat(&self, path: &[u8], follow: bool) -> HostFuture<io::Result<Metadata>> {
        let fut = op(sys::K_STAT, u32::from(follow), path);
        Box::pin(async move { parse_stat(&fut.await.result()?.data) })
    }

    fn access(&self, path: &[u8], mode: Access) -> HostFuture<io::Result<bool>> {
        let m = match mode {
            Access::Read => 0,
            Access::Write => 1,
            Access::Execute => 2,
        };
        let fut = op(sys::K_ACCESS, m, path);
        Box::pin(async move { Ok(fut.await.result()?.value != 0.0) })
    }

    fn read_dir(&self, path: &[u8]) -> HostFuture<io::Result<Vec<DirEntry>>> {
        let fut = op(sys::K_READ_DIR, 0, path);
        Box::pin(async move {
            let a = fut.await.result()?;
            Ok(a.data
                .split(|&b| b == 0)
                .filter(|n| !n.is_empty())
                .map(|n| DirEntry { name: n.to_vec() })
                .collect())
        })
    }

    fn canonicalize(&self, path: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        let fut = op(sys::K_CANONICALIZE, 0, path);
        Box::pin(async move { Ok(fut.await.result()?.data) })
    }

    fn resolve(&self, name: &[u8], path: &[u8], cwd: &[u8]) -> HostFuture<Option<Vec<u8>>> {
        let mut w = Writer::new();
        w.bytes(name).bytes(path).bytes(cwd);
        let fut = op(sys::K_RESOLVE, 0, &w.0);
        Box::pin(async move { fut.await.result().ok().map(|a| a.data) })
    }

    fn spawn(&self, spec: SpawnSpec) -> io::Result<Spawned> {
        let mut w = Writer::new();
        w.bytes(&spec.program).u32(spec.argv.len() as u32);
        for a in &spec.argv {
            w.bytes(a);
        }
        w.u32(spec.env.len() as u32);
        for (k, v) in &spec.env {
            w.bytes(k).bytes(v);
        }
        w.bytes(&spec.cwd).u32(spec.fds.len() as u32);
        for (n, fd) in &spec.fds {
            w.u32(*n);
            match fd {
                ChildFd::File(f) => {
                    let any: &dyn std::any::Any = f.as_ref();
                    let Some(js) = any.downcast_ref::<JsFile>() else {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "a descriptor the JS host does not know",
                        ));
                    };
                    w.u32(FD_FILE).u32(js.handle);
                }
                ChildFd::Pipe { input: true } => {
                    w.u32(FD_PIPE_IN).u32(0);
                }
                ChildFd::Pipe { input: false } => {
                    w.u32(FD_PIPE_OUT).u32(0);
                }
                ChildFd::Dup(m) => {
                    w.u32(FD_DUP).u32(*m);
                }
                ChildFd::Null => {
                    w.u32(FD_NULL).u32(0);
                }
            }
        }
        let id = ops::start(sys::K_SPAWN, 0, &w.0);
        let a = ops::take_now(id)
            .ok_or_else(|| io::Error::other("the JS host did not answer spawn at once"))?
            .result()?;
        // The engine's ends of the pipes: `fd handle` pairs, as text.
        let text = String::from_utf8_lossy(&a.data).into_owned();
        let mut pipes = Vec::new();
        for pair in text.split_whitespace() {
            let (fd, handle) = pair
                .split_once(':')
                .and_then(|(f, h)| Some((f.parse().ok()?, h.parse().ok()?)))
                .ok_or_else(|| io::Error::other("bad spawn answer"))?;
            pipes.push((fd, Rc::new(JsFile::new(handle)) as Rc<dyn HostFile>));
        }
        Ok(Spawned {
            proc: Box::new(JsProc {
                pid: a.value as u32,
                wait: None,
                released: false,
            }),
            pipes,
        })
    }

    fn sleep(&self, duration: Duration) -> HostFuture<()> {
        let ms = duration.as_millis().min(u32::MAX as u128) as u32;
        let fut = op(sys::K_SLEEP, ms, &[]);
        Box::pin(async move {
            fut.await;
        })
    }

    fn pid(&self) -> u32 {
        sys::call(sys::K_PID, 0, 0, &[]) as u32
    }

    fn stat_extra(
        &self,
        path: &[u8],
        follow: bool,
    ) -> HostFuture<io::Result<(Metadata, ExtraMetadata)>> {
        let fut = op(
            sys::K_FS,
            sys::FS_STAT_EXTRA,
            &path_mode(path, u32::from(follow)).0,
        );
        Box::pin(async move { parse_stat_extra(&fut.await.result()?.data) })
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        fs_unit(sys::FS_MKDIR, &path_mode(path, mode))
    }

    fn remove_file(&self, path: &[u8]) -> HostFuture<io::Result<()>> {
        fs_unit(sys::FS_REMOVE_FILE, &paths(path, b""))
    }

    fn remove_dir(&self, path: &[u8]) -> HostFuture<io::Result<()>> {
        fs_unit(sys::FS_REMOVE_DIR, &paths(path, b""))
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> HostFuture<io::Result<()>> {
        fs_unit(sys::FS_RENAME, &paths(from, to))
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        fs_unit(sys::FS_SYMLINK, &paths(target, link))
    }

    fn hard_link(&self, existing: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        fs_unit(sys::FS_HARD_LINK, &paths(existing, link))
    }

    fn read_link(&self, path: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        let fut = op(sys::K_FS, sys::FS_READ_LINK, &paths(path, b"").0);
        Box::pin(async move { Ok(fut.await.result()?.data) })
    }

    fn set_mode(&self, path: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        fs_unit(sys::FS_SET_MODE, &path_mode(path, mode))
    }

    fn set_times(
        &self,
        path: &[u8],
        atime_ns: Option<i64>,
        mtime_ns: Option<i64>,
        follow: bool,
    ) -> HostFuture<io::Result<()>> {
        let flags = u32::from(atime_ns.is_some())
            | u32::from(mtime_ns.is_some()) << 1
            | u32::from(follow) << 2;
        let mut w = Writer::new();
        w.bytes(path)
            .u32(flags)
            .bytes(atime_ns.unwrap_or(0).to_string().as_bytes())
            .bytes(mtime_ns.unwrap_or(0).to_string().as_bytes());
        fs_unit(sys::FS_SET_TIMES, &w)
    }

    fn user_name(&self, uid: u32) -> Option<Vec<u8>> {
        name_of(0, uid)
    }

    fn group_name(&self, gid: u32) -> Option<Vec<u8>> {
        name_of(1, gid)
    }

    fn utc_offset(&self, epoch_secs: i64) -> i32 {
        sys::call(sys::K_UTC_OFFSET, 0, 0, epoch_secs.to_string().as_bytes())
    }

    fn now(&self) -> Duration {
        let n = sys::call(sys::K_NOW, 0, 0, &[]);
        let data = crate::take_input(n.max(0) as usize);
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
}

/// `stat`'s answer: `kind mode size mtime_ns dev ino uid gid` as decimal text; kind is
/// one of `file dir symlink fifo socket block char other`.
fn parse_stat(data: &[u8]) -> io::Result<Metadata> {
    let text = String::from_utf8_lossy(data);
    let f: Vec<&str> = text.split_whitespace().collect();
    if f.len() != 8 {
        return Err(bad_stat());
    }
    stat_fields(&f)
}

fn bad_stat() -> io::Error {
    io::Error::other("bad stat answer")
}

/// A decimal field: an integer (exact nanoseconds), else a float.
fn num(s: &str) -> io::Result<f64> {
    s.parse::<f64>().map_err(|_| bad_stat())
}

fn int(s: &str) -> io::Result<i64> {
    s.parse::<i64>().or_else(|_| num(s).map(|v| v as i64))
}

/// `stat_extra`'s answer: `stat`'s eight fields, then `atime_ns ctime_ns btime_ns|-
/// nlink blocks blksize rdev`.
fn parse_stat_extra(data: &[u8]) -> io::Result<(Metadata, ExtraMetadata)> {
    let text = String::from_utf8_lossy(data);
    let f: Vec<&str> = text.split_whitespace().collect();
    if f.len() != 15 {
        return Err(bad_stat());
    }
    let meta = stat_fields(&f[..8])?;
    let extra = ExtraMetadata {
        atime_ns: int(f[8])?,
        ctime_ns: int(f[9])?,
        btime_ns: if f[10] == "-" {
            None
        } else {
            Some(int(f[10])?)
        },
        nlink: int(f[11])? as u64,
        blocks: int(f[12])? as u64,
        blksize: int(f[13])? as u64,
        rdev: int(f[14])? as u64,
    };
    Ok((meta, extra))
}

fn stat_fields(f: &[&str]) -> io::Result<Metadata> {
    let n = |i: usize| num(f[i]);
    let kind = match f[0] {
        "file" => FileKind::File,
        "dir" => FileKind::Dir,
        "symlink" => FileKind::Symlink,
        "fifo" => FileKind::Fifo,
        "socket" => FileKind::Socket,
        "block" => FileKind::BlockDevice,
        "char" => FileKind::CharDevice,
        _ => FileKind::Other,
    };
    Ok(Metadata {
        kind,
        mode: n(1)? as u32,
        size: n(2)? as u64,
        mtime_ns: int(f[3])?,
        dev: n(4)? as u64,
        ino: n(5)? as u64,
        uid: n(6)? as u32,
        gid: n(7)? as u32,
    })
}

/// A file, pipe end or device of the JS host.
pub struct JsFile {
    pub handle: u32,
    /// The read in flight, and bytes read beyond what the last caller could take.
    read: Cell<Option<u32>>,
    spill: RefCell<Vec<u8>>,
    write: Cell<Option<u32>>,
}

impl JsFile {
    pub fn new(handle: u32) -> JsFile {
        JsFile {
            handle,
            read: Cell::new(None),
            spill: RefCell::new(Vec::new()),
            write: Cell::new(None),
        }
    }
}

impl HostFile for JsFile {
    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        {
            let mut spill = self.spill.borrow_mut();
            if !spill.is_empty() {
                let n = spill.len().min(buf.len());
                buf[..n].copy_from_slice(&spill[..n]);
                spill.drain(..n);
                return Poll::Ready(Ok(n));
            }
        }
        let id = match self.read.get() {
            Some(id) => id,
            None => {
                let id = ops::start_len(sys::K_READ, self.handle, buf.len().max(1));
                self.read.set(Some(id));
                id
            }
        };
        match ops::poll(id, cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(a) => {
                self.read.set(None);
                let a = match a.result() {
                    Ok(a) => a,
                    Err(e) => return Poll::Ready(Err(e)),
                };
                let n = a.data.len().min(buf.len());
                buf[..n].copy_from_slice(&a.data[..n]);
                self.spill.borrow_mut().extend_from_slice(&a.data[n..]);
                Poll::Ready(Ok(n))
            }
        }
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let id = match self.write.get() {
            Some(id) => id,
            None => {
                let id = ops::start(sys::K_WRITE, self.handle, buf);
                self.write.set(Some(id));
                id
            }
        };
        match ops::poll(id, cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(a) => {
                self.write.set(None);
                Poll::Ready(a.result().map(|a| a.value as usize))
            }
        }
    }
}

impl Drop for JsFile {
    fn drop(&mut self) {
        for op in [self.read.get(), self.write.get()].into_iter().flatten() {
            ops::forget(op);
        }
        sys::call(sys::K_CLOSE, 0, self.handle, &[]);
    }
}

/// A command the JS host runs.
struct JsProc {
    pid: u32,
    wait: Option<u32>,
    released: bool,
}

impl ExtProc for JsProc {
    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<ExitStatus> {
        let id = *self
            .wait
            .get_or_insert_with(|| ops::start(sys::K_WAIT, self.pid, &[]));
        match ops::poll(id, cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(a) => {
                self.wait = None;
                // A negative value: killed by that signal.
                Poll::Ready(if a.value < 0.0 {
                    ExitStatus::Signaled(-a.value as Signal)
                } else {
                    ExitStatus::Exited(a.value as i32)
                })
            }
        }
    }

    fn signal(&mut self, sig: Signal) {
        sys::call_len(sys::K_SIGNAL, 0, self.pid, std::ptr::null(), sig as usize);
    }

    fn release(mut self: Box<Self>) {
        self.released = true;
        sys::call(sys::K_RELEASE, 0, self.pid, &[]);
    }
}

impl Drop for JsProc {
    fn drop(&mut self) {
        if let Some(op) = self.wait.take() {
            ops::forget(op);
        }
        if !self.released {
            sys::call(sys::K_ABANDON, 0, self.pid, &[]);
        }
    }
}
