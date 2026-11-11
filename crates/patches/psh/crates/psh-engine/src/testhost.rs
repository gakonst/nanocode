//! A deterministic host for tests: an in-memory file system, fake external commands
//! written as futures, and captured stdio. No OS, no threads.
//!
//! In latency mode every operation is `Pending` once before it completes (the waker is
//! woken at once), which proves that no engine path expects a synchronous answer
//! (ARCH.md "Hosts", TESTING.md L6). Counters of live processes, open files and pipes
//! let tests check that a cancel leaks nothing.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::{Future, poll_fn};
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::host::{
    Access, ChildFd, DirEntry, ExitStatus, ExtProc, ExtraMetadata, FileKind, Host, HostFile,
    HostFuture, Metadata, OpenOptions, Signal, SpawnSpec, Spawned,
};

/// What a fake command gets.
pub struct FakeProc {
    pub argv: Vec<Vec<u8>>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub cwd: Vec<u8>,
    pub stdin: Rc<dyn HostFile>,
    pub stdout: Rc<dyn HostFile>,
    pub stderr: Rc<dyn HostFile>,
    pub fs: Rc<MemFs>,
}

impl FakeProc {
    pub async fn read_all(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match poll_fn(|cx| self.stdin.poll_read(cx, &mut buf)).await {
                Ok(0) | Err(_) => return out,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
    }

    /// Reads one line (with its newline); empty at end of input.
    pub async fn read_line(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut b = [0u8; 1];
        loop {
            match poll_fn(|cx| self.stdin.poll_read(cx, &mut b)).await {
                Ok(0) | Err(_) => return out,
                Ok(_) => {
                    out.push(b[0]);
                    if b[0] == b'\n' {
                        return out;
                    }
                }
            }
        }
    }

    pub async fn write(&self, data: &[u8]) -> io::Result<()> {
        write_all(&*self.stdout, data).await
    }

    pub async fn write_err(&self, data: &[u8]) -> io::Result<()> {
        write_all(&*self.stderr, data).await
    }
}

async fn write_all(f: &dyn HostFile, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let n = poll_fn(|cx| f.poll_write(cx, data)).await?;
        data = &data[n..];
    }
    Ok(())
}

pub type FakeCommand = Rc<dyn Fn(FakeProc) -> Pin<Box<dyn Future<Output = i32>>>>;

/// Counts of live resources.
#[derive(Default)]
struct Counters {
    procs: Cell<usize>,
    /// Pids handed to fakes (`ExtProc::pid`).
    spawned: Cell<u32>,
    files: Cell<usize>,
    pipes: Cell<usize>,
}

/// Pending once, then ready: the latency mode's answer to every operation.
struct Delay {
    on: bool,
}

impl Delay {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.on {
            self.on = false;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

fn delayed<T: 'static>(latency: bool, value: T) -> HostFuture<T> {
    let mut d = Delay { on: latency };
    let mut v = Some(value);
    Box::pin(poll_fn(move |cx| match d.poll(cx) {
        Poll::Ready(()) => Poll::Ready(v.take().expect("polled after completion")),
        Poll::Pending => Poll::Pending,
    }))
}

/// Toggles between `Pending` and ready for every call when latency is on.
struct Toggle {
    latency: bool,
    next_pending: Cell<bool>,
}

impl Toggle {
    fn new(latency: bool) -> Toggle {
        Toggle {
            latency,
            next_pending: Cell::new(true),
        }
    }

    fn gate(&self, cx: &mut Context<'_>) -> bool {
        if !self.latency {
            return true;
        }
        if self.next_pending.get() {
            self.next_pending.set(false);
            cx.waker().wake_by_ref();
            false
        } else {
            self.next_pending.set(true);
            true
        }
    }
}

enum Node {
    File {
        data: Rc<RefCell<Vec<u8>>>,
        mode: u32,
        mtime: i64,
        atime: i64,
        ino: u64,
    },
    Dir {
        mode: u32,
        mtime: i64,
        ino: u64,
    },
    Symlink {
        target: Vec<u8>,
        mtime: i64,
        ino: u64,
    },
}

impl Node {
    fn ino(&self) -> u64 {
        match self {
            Node::File { ino, .. } | Node::Dir { ino, .. } | Node::Symlink { ino, .. } => *ino,
        }
    }

    fn is_dir(&self) -> bool {
        matches!(self, Node::Dir { .. })
    }
}

/// An in-memory file system: absolute `/`-separated paths, directories, files and
/// symlinks (resolved physically, `..` included). Times come from a clock that ticks
/// once per change.
pub struct MemFs {
    nodes: RefCell<BTreeMap<Vec<u8>, Node>>,
    clock: Cell<i64>,
    next_ino: Cell<u64>,
    /// The file system changes of D1150 answer `Unsupported` (a host without them).
    unsupported: Cell<bool>,
}

fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(0) => b"/",
        Some(i) => &path[..i],
        None => b"/",
    }
}

fn clean(path: &[u8]) -> Vec<u8> {
    crate::builtins::normalize(path)
}

fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = dir.to_vec();
    if p != b"/" {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

fn err<T>(kind: io::ErrorKind) -> io::Result<T> {
    Err(kind.into())
}

impl MemFs {
    fn new() -> MemFs {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            b"/".to_vec(),
            Node::Dir {
                mode: 0o755,
                mtime: 0,
                ino: 1,
            },
        );
        MemFs {
            nodes: RefCell::new(nodes),
            clock: Cell::new(1),
            next_ino: Cell::new(2),
            unsupported: Cell::new(false),
        }
    }

    fn ino(&self) -> u64 {
        let i = self.next_ino.get();
        self.next_ino.set(i + 1);
        i
    }

    fn new_dir(&self, mode: u32) -> Node {
        Node::Dir {
            mode,
            mtime: self.tick(),
            ino: self.ino(),
        }
    }

    pub fn mkdir_all(&self, path: &str) {
        let p = clean(path.as_bytes());
        let mut cur = Vec::new();
        for c in p.split(|&b| b == b'/').filter(|c| !c.is_empty()) {
            cur.push(b'/');
            cur.extend_from_slice(c);
            if !self.nodes.borrow().contains_key(&cur) {
                let d = self.new_dir(0o755);
                self.nodes.borrow_mut().insert(cur.clone(), d);
            }
        }
    }

    pub fn write_file(&self, path: &str, data: &[u8]) {
        let p = clean(path.as_bytes());
        self.mkdir_all(&String::from_utf8_lossy(parent(&p)));
        let t = self.tick();
        let ino = self.ino();
        self.nodes.borrow_mut().insert(
            p,
            Node::File {
                data: Rc::new(RefCell::new(data.to_vec())),
                mode: 0o644,
                mtime: t,
                atime: t,
                ino,
            },
        );
    }

    /// Adds a symlink (its parent directories are made).
    pub fn symlink(&self, target: &str, path: &str) {
        let p = clean(path.as_bytes());
        self.mkdir_all(&String::from_utf8_lossy(parent(&p)));
        let mtime = self.tick();
        let ino = self.ino();
        self.nodes.borrow_mut().insert(
            p,
            Node::Symlink {
                target: target.as_bytes().to_vec(),
                mtime,
                ino,
            },
        );
    }

    pub fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let p = self.resolve(path.as_bytes(), true).ok()?;
        match self.nodes.borrow().get(&p) {
            Some(Node::File { data, .. }) => Some(data.borrow().clone()),
            _ => None,
        }
    }

    /// Whether a path exists (a symlink itself counts).
    pub fn exists(&self, path: &str) -> bool {
        self.resolve(path.as_bytes(), false)
            .is_ok_and(|p| self.nodes.borrow().contains_key(&p))
    }

    /// Makes the file system changes of D1150 answer `Unsupported`, as a host without
    /// them does.
    pub fn set_unsupported(&self, on: bool) {
        self.unsupported.set(on);
    }

    fn tick(&self) -> i64 {
        self.clock.set(self.clock.get() + 1);
        self.clock.get()
    }

    /// The physical path of `path`: symlinks in its directories resolved, and the last
    /// component too when `follow`. The result need not exist; its parent does.
    fn resolve(&self, path: &[u8], follow: bool) -> io::Result<Vec<u8>> {
        self.resolve_in(path, follow, 0)
    }

    fn resolve_in(&self, path: &[u8], follow: bool, depth: u32) -> io::Result<Vec<u8>> {
        if depth > 40 {
            return Err(io::Error::from_raw_os_error(40));
        }
        let parts: Vec<&[u8]> = path
            .split(|&b| b == b'/')
            .filter(|c| !c.is_empty() && *c != b".")
            .collect();
        let mut cur = b"/".to_vec();
        for (i, c) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            if *c == b".." {
                cur = parent(&cur).to_vec();
                continue;
            }
            let next = join(&cur, c);
            let nodes = self.nodes.borrow();
            match nodes.get(&next) {
                Some(Node::Symlink { target, .. }) if !last || follow => {
                    let mut t = if target.first() == Some(&b'/') {
                        target.clone()
                    } else {
                        join(&cur, target)
                    };
                    for rest in &parts[i + 1..] {
                        t.push(b'/');
                        t.extend_from_slice(rest);
                    }
                    drop(nodes);
                    return self.resolve_in(&t, follow, depth + 1);
                }
                Some(Node::Dir { .. }) => cur = next,
                Some(_) if !last => return err(io::ErrorKind::NotADirectory),
                None if !last => return err(io::ErrorKind::NotFound),
                _ => cur = next,
            }
        }
        Ok(cur)
    }

    /// The existing parent directory of a path that is to be created or removed, and
    /// the path with its last component unresolved.
    fn entry_path(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        let p = self.resolve(path, false)?;
        match self.nodes.borrow().get(parent(&p)) {
            Some(Node::Dir { .. }) => Ok(p),
            Some(_) => err(io::ErrorKind::NotADirectory),
            None => err(io::ErrorKind::NotFound),
        }
    }

    fn touch_dir(&self, dir: &[u8]) {
        let t = self.tick();
        if let Some(Node::Dir { mtime, .. }) = self.nodes.borrow_mut().get_mut(dir) {
            *mtime = t;
        }
    }

    fn check(&self) -> io::Result<()> {
        if self.unsupported.get() {
            return err(io::ErrorKind::Unsupported);
        }
        Ok(())
    }

    fn stat(&self, path: &[u8], follow: bool) -> io::Result<Metadata> {
        Ok(self.stat_extra(path, follow)?.0)
    }

    fn stat_extra(&self, path: &[u8], follow: bool) -> io::Result<(Metadata, ExtraMetadata)> {
        let p = self.resolve(path, follow)?;
        let nodes = self.nodes.borrow();
        let node = nodes.get(&p).ok_or(io::ErrorKind::NotFound)?;
        let (kind, size, mode, mtime, atime) = match node {
            Node::Dir { mode, mtime, .. } => (FileKind::Dir, 4096, *mode, *mtime, *mtime),
            Node::File {
                data,
                mode,
                mtime,
                atime,
                ..
            } => (
                FileKind::File,
                data.borrow().len() as u64,
                *mode,
                *mtime,
                *atime,
            ),
            Node::Symlink { target, mtime, .. } => (
                FileKind::Symlink,
                target.len() as u64,
                0o777,
                *mtime,
                *mtime,
            ),
        };
        let ino = node.ino();
        let nlink = match node {
            Node::Dir { .. } => {
                2 + self
                    .children(&nodes, &p)
                    .filter(|k| nodes.get(*k).is_some_and(Node::is_dir))
                    .count() as u64
            }
            _ => nodes.values().filter(|n| n.ino() == ino).count() as u64,
        };
        let meta = Metadata {
            kind,
            size,
            mode,
            mtime_ns: mtime,
            dev: 1,
            ino,
            uid: 1000,
            gid: 1000,
        };
        let extra = ExtraMetadata {
            atime_ns: atime,
            ctime_ns: mtime,
            btime_ns: None,
            nlink,
            blocks: size.div_ceil(4096) * 8,
            blksize: 4096,
            rdev: 0,
        };
        Ok((meta, extra))
    }

    fn children<'a>(
        &self,
        nodes: &'a BTreeMap<Vec<u8>, Node>,
        dir: &[u8],
    ) -> impl Iterator<Item = &'a Vec<u8>> + 'a {
        let mut prefix = dir.to_vec();
        if prefix != b"/" {
            prefix.push(b'/');
        }
        nodes.keys().filter(move |k| {
            k.starts_with(&prefix) && k.len() > prefix.len() && !k[prefix.len()..].contains(&b'/')
        })
    }

    fn open(&self, path: &[u8], opts: OpenOptions) -> io::Result<Rc<RefCell<Vec<u8>>>> {
        let p = self.resolve(path, true)?;
        let mut nodes = self.nodes.borrow_mut();
        match nodes.get_mut(&p) {
            Some(Node::Dir { .. }) => {
                return Err(io::ErrorKind::IsADirectory.into());
            }
            Some(Node::File {
                data, mtime, atime, ..
            }) => {
                if opts.create_new {
                    return Err(io::ErrorKind::AlreadyExists.into());
                }
                let t = self.clock.get() + 1;
                self.clock.set(t);
                if opts.truncate {
                    data.borrow_mut().clear();
                }
                if opts.write || opts.append {
                    *mtime = t;
                } else {
                    *atime = t;
                }
                return Ok(data.clone());
            }
            Some(Node::Symlink { .. }) => return err(io::ErrorKind::NotFound),
            None => {}
        }
        if !opts.create {
            return Err(io::ErrorKind::NotFound.into());
        }
        match nodes.get(parent(&p)) {
            Some(Node::Dir { .. }) => {}
            Some(_) => return Err(io::ErrorKind::NotADirectory.into()),
            None => return Err(io::ErrorKind::NotFound.into()),
        }
        let data = Rc::new(RefCell::new(Vec::new()));
        let t = self.clock.get() + 1;
        self.clock.set(t);
        let ino = self.ino();
        nodes.insert(
            p,
            Node::File {
                data: data.clone(),
                mode: opts.mode.unwrap_or(0o644) & 0o7777,
                mtime: t,
                atime: t,
                ino,
            },
        );
        Ok(data)
    }

    fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
        let p = self.resolve(path, true)?;
        let nodes = self.nodes.borrow();
        match nodes.get(&p) {
            Some(Node::Dir { .. }) => {}
            Some(_) => return Err(io::ErrorKind::NotADirectory.into()),
            None => return Err(io::ErrorKind::NotFound.into()),
        }
        let skip = if p == b"/" { 1 } else { p.len() + 1 };
        Ok(self
            .children(&nodes, &p)
            .map(|k| DirEntry {
                name: k[skip..].to_vec(),
            })
            .collect())
    }

    fn canonicalize(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        let p = self.resolve(path, true)?;
        if !self.nodes.borrow().contains_key(&p) {
            return err(io::ErrorKind::NotFound);
        }
        Ok(p)
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> io::Result<()> {
        self.check()?;
        let p = self.entry_path(path)?;
        if self.nodes.borrow().contains_key(&p) {
            return err(io::ErrorKind::AlreadyExists);
        }
        let d = self.new_dir(mode & 0o7777);
        self.nodes.borrow_mut().insert(p.clone(), d);
        self.touch_dir(parent(&p));
        Ok(())
    }

    fn remove(&self, path: &[u8], dir: bool) -> io::Result<()> {
        self.check()?;
        let p = self.entry_path(path)?;
        let mut nodes = self.nodes.borrow_mut();
        match nodes.get(&p) {
            None => return err(io::ErrorKind::NotFound),
            Some(Node::Dir { .. }) if !dir => return err(io::ErrorKind::IsADirectory),
            Some(Node::Dir { .. }) => {
                if p == b"/" {
                    return err(io::ErrorKind::ResourceBusy);
                }
                if self.children(&nodes, &p).next().is_some() {
                    return err(io::ErrorKind::DirectoryNotEmpty);
                }
            }
            Some(_) if dir => return err(io::ErrorKind::NotADirectory),
            Some(_) => {}
        }
        nodes.remove(&p);
        drop(nodes);
        self.touch_dir(parent(&p));
        Ok(())
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        self.check()?;
        let from = self.entry_path(from)?;
        let to = self.entry_path(to)?;
        let mut nodes = self.nodes.borrow_mut();
        let Some(src) = nodes.get(&from) else {
            return err(io::ErrorKind::NotFound);
        };
        if from == to {
            return Ok(());
        }
        let src_dir = src.is_dir();
        if src_dir && to.starts_with(&from) && to.get(from.len()) == Some(&b'/') {
            return err(io::ErrorKind::InvalidInput);
        }
        match nodes.get(&to) {
            Some(Node::Dir { .. }) if !src_dir => return err(io::ErrorKind::IsADirectory),
            Some(Node::Dir { .. }) => {
                if self.children(&nodes, &to).next().is_some() {
                    return err(io::ErrorKind::DirectoryNotEmpty);
                }
            }
            Some(_) if src_dir => return err(io::ErrorKind::NotADirectory),
            _ => {}
        }
        // The node and everything below it move.
        let mut prefix = from.clone();
        prefix.push(b'/');
        let moved: Vec<Vec<u8>> = nodes
            .keys()
            .filter(|k| **k == from || k.starts_with(&prefix))
            .cloned()
            .collect();
        nodes.remove(&to);
        for k in moved {
            let node = nodes.remove(&k).expect("listed");
            let mut nk = to.clone();
            nk.extend_from_slice(&k[from.len()..]);
            nodes.insert(nk, node);
        }
        drop(nodes);
        self.touch_dir(parent(&from));
        self.touch_dir(parent(&to));
        Ok(())
    }

    fn symlink_at(&self, target: &[u8], link: &[u8]) -> io::Result<()> {
        self.check()?;
        let p = self.entry_path(link)?;
        if self.nodes.borrow().contains_key(&p) {
            return err(io::ErrorKind::AlreadyExists);
        }
        let mtime = self.tick();
        let ino = self.ino();
        self.nodes.borrow_mut().insert(
            p.clone(),
            Node::Symlink {
                target: target.to_vec(),
                mtime,
                ino,
            },
        );
        self.touch_dir(parent(&p));
        Ok(())
    }

    fn hard_link(&self, existing: &[u8], link: &[u8]) -> io::Result<()> {
        self.check()?;
        let src = self.resolve(existing, false)?;
        let p = self.entry_path(link)?;
        let mut nodes = self.nodes.borrow_mut();
        if nodes.contains_key(&p) {
            return err(io::ErrorKind::AlreadyExists);
        }
        let node = match nodes.get(&src) {
            None => return err(io::ErrorKind::NotFound),
            Some(Node::Dir { .. }) => return err(io::ErrorKind::PermissionDenied),
            Some(Node::File {
                data,
                mode,
                mtime,
                atime,
                ino,
            }) => Node::File {
                data: data.clone(),
                mode: *mode,
                mtime: *mtime,
                atime: *atime,
                ino: *ino,
            },
            Some(Node::Symlink { target, mtime, ino }) => Node::Symlink {
                target: target.clone(),
                mtime: *mtime,
                ino: *ino,
            },
        };
        nodes.insert(p, node);
        Ok(())
    }

    fn read_link(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        self.check()?;
        let p = self.resolve(path, false)?;
        match self.nodes.borrow().get(&p) {
            Some(Node::Symlink { target, .. }) => Ok(target.clone()),
            Some(_) => err(io::ErrorKind::InvalidInput),
            None => err(io::ErrorKind::NotFound),
        }
    }

    fn set_mode(&self, path: &[u8], new: u32) -> io::Result<()> {
        self.check()?;
        let p = self.resolve(path, true)?;
        match self.nodes.borrow_mut().get_mut(&p) {
            Some(Node::File { mode, .. } | Node::Dir { mode, .. }) => {
                *mode = new & 0o7777;
                Ok(())
            }
            _ => err(io::ErrorKind::NotFound),
        }
    }

    fn set_times(
        &self,
        path: &[u8],
        at: Option<i64>,
        mt: Option<i64>,
        follow: bool,
    ) -> io::Result<()> {
        self.check()?;
        let p = self.resolve(path, follow)?;
        match self.nodes.borrow_mut().get_mut(&p) {
            Some(Node::File { mtime, atime, .. }) => {
                if let Some(a) = at {
                    *atime = a;
                }
                if let Some(m) = mt {
                    *mtime = m;
                }
                Ok(())
            }
            Some(Node::Dir { mtime, .. } | Node::Symlink { mtime, .. }) => {
                if let Some(m) = mt {
                    *mtime = m;
                }
                Ok(())
            }
            None => err(io::ErrorKind::NotFound),
        }
    }
}

/// An open file of the memory fs, or a capture buffer.
struct MemFile {
    data: Rc<RefCell<Vec<u8>>>,
    pos: Cell<usize>,
    read: bool,
    write: bool,
    append: bool,
    toggle: Toggle,
    counters: Rc<Counters>,
}

impl Drop for MemFile {
    fn drop(&mut self) {
        self.counters.files.set(self.counters.files.get() - 1);
    }
}

impl HostFile for MemFile {
    fn kind(&self) -> Option<crate::host::FileKind> {
        Some(crate::host::FileKind::File)
    }

    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if !self.toggle.gate(cx) {
            return Poll::Pending;
        }
        if !self.read {
            return Poll::Ready(Err(crate::fd::bad_fd()));
        }
        let data = self.data.borrow();
        let pos = self.pos.get().min(data.len());
        let n = buf.len().min(data.len() - pos);
        buf[..n].copy_from_slice(&data[pos..pos + n]);
        self.pos.set(pos + n);
        Poll::Ready(Ok(n))
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if !self.toggle.gate(cx) {
            return Poll::Pending;
        }
        if !self.write {
            return Poll::Ready(Err(crate::fd::bad_fd()));
        }
        let mut data = self.data.borrow_mut();
        let pos = if self.append {
            data.len()
        } else {
            self.pos.get()
        };
        if data.len() < pos + buf.len() {
            data.resize(pos + buf.len(), 0);
        }
        data[pos..pos + buf.len()].copy_from_slice(buf);
        self.pos.set(pos + buf.len());
        Poll::Ready(Ok(buf.len()))
    }
}

struct TPipeInner {
    buf: VecDeque<u8>,
    reader: bool,
    writer: bool,
    waker: Option<Waker>,
    wwaker: Option<Waker>,
}

/// One end of a pipe between the engine and a fake process.
struct TPipe {
    inner: Rc<RefCell<TPipeInner>>,
    read_end: bool,
    toggle: Toggle,
    counters: Rc<Counters>,
}

impl Drop for TPipe {
    fn drop(&mut self) {
        let mut p = self.inner.borrow_mut();
        if self.read_end {
            p.reader = false;
            if let Some(w) = p.wwaker.take() {
                w.wake();
            }
        } else {
            p.writer = false;
            if let Some(w) = p.waker.take() {
                w.wake();
            }
        }
        if !p.reader && !p.writer {
            self.counters.pipes.set(self.counters.pipes.get() - 1);
        }
    }
}

impl HostFile for TPipe {
    fn kind(&self) -> Option<crate::host::FileKind> {
        Some(crate::host::FileKind::Fifo)
    }

    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if !self.read_end {
            return Poll::Ready(Err(crate::fd::bad_fd()));
        }
        if !self.toggle.gate(cx) {
            return Poll::Pending;
        }
        let mut p = self.inner.borrow_mut();
        if !p.buf.is_empty() {
            let n = buf.len().min(p.buf.len());
            for (i, b) in p.buf.drain(..n).enumerate() {
                buf[i] = b;
            }
            if let Some(w) = p.wwaker.take() {
                w.wake();
            }
            return Poll::Ready(Ok(n));
        }
        if !p.writer {
            return Poll::Ready(Ok(0));
        }
        p.waker = Some(cx.waker().clone());
        Poll::Pending
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.read_end {
            return Poll::Ready(Err(crate::fd::bad_fd()));
        }
        if !self.toggle.gate(cx) {
            return Poll::Pending;
        }
        let mut p = self.inner.borrow_mut();
        if !p.reader {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        // A small buffer, so that back pressure is exercised.
        let space = 4096usize.saturating_sub(p.buf.len());
        if space == 0 {
            p.wwaker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = space.min(buf.len());
        p.buf.extend(&buf[..n]);
        if let Some(w) = p.waker.take() {
            w.wake();
        }
        Poll::Ready(Ok(n))
    }
}

struct NullFile;

impl HostFile for NullFile {
    fn kind(&self) -> Option<crate::host::FileKind> {
        Some(crate::host::FileKind::CharDevice)
    }

    fn poll_read(&self, _cx: &mut Context<'_>, _buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(0))
    }
    fn poll_write(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }
}

struct FakeExt {
    pid: u32,
    /// The waiter to wake when a signal ends the fake.
    waker: Option<std::task::Waker>,
    fut: Option<Pin<Box<dyn Future<Output = i32>>>>,
    status: Option<ExitStatus>,
    delay: Delay,
    counters: Rc<Counters>,
}

impl ExtProc for FakeExt {
    fn pid(&self) -> Option<u32> {
        Some(self.pid)
    }

    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<ExitStatus> {
        if self.delay.poll(cx).is_pending() {
            return Poll::Pending;
        }
        if let Some(s) = self.status {
            return Poll::Ready(s);
        }
        let Some(fut) = self.fut.as_mut() else {
            return Poll::Ready(ExitStatus::Signaled(crate::host::SIGKILL));
        };
        match fut.as_mut().poll(cx) {
            Poll::Ready(code) => {
                self.fut = None;
                self.status = Some(ExitStatus::Exited(code));
                Poll::Ready(ExitStatus::Exited(code))
            }
            Poll::Pending => {
                self.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    fn release(self: Box<Self>) {
        // A fake cannot run without being polled: released, it simply stops.
    }

    fn signal(&mut self, sig: Signal) {
        if self.status.is_none() {
            // The fake dies at once: dropping its future closes its pipes.
            self.fut = None;
            self.status = Some(ExitStatus::Signaled(sig));
            if let Some(w) = self.waker.take() {
                w.wake();
            }
        }
    }
}

impl Drop for FakeExt {
    fn drop(&mut self) {
        self.counters.procs.set(self.counters.procs.get() - 1);
    }
}

/// The test host. Create with [`TestHost::new`], add commands and files, then make a
/// shell with [`TestHost::config`].
pub struct TestHost {
    pub fs: Rc<MemFs>,
    commands: RefCell<HashMap<Vec<u8>, FakeCommand>>,
    latency: Cell<bool>,
    /// What `path_entries` splits on (`;` plays a Windows host).
    path_sep: Cell<u8>,
    strips_crlf: Cell<bool>,
    counters: Rc<Counters>,
    stdin: Rc<RefCell<Vec<u8>>>,
    stdout: Rc<RefCell<Vec<u8>>>,
    stderr: Rc<RefCell<Vec<u8>>>,
}

impl Default for TestHost {
    fn default() -> Self {
        TestHost::new()
    }
}

impl TestHost {
    /// A host with `/tmp`, the standard fake commands in `/bin` and empty stdio.
    pub fn new() -> TestHost {
        let fs = Rc::new(MemFs::new());
        fs.mkdir_all("/tmp");
        fs.mkdir_all("/bin");
        let host = TestHost {
            fs,
            commands: RefCell::new(HashMap::new()),
            latency: Cell::new(false),
            path_sep: Cell::new(b':'),
            strips_crlf: Cell::new(false),
            counters: Rc::new(Counters::default()),
            stdin: Rc::new(RefCell::new(Vec::new())),
            stdout: Rc::new(RefCell::new(Vec::new())),
            stderr: Rc::new(RefCell::new(Vec::new())),
        };
        add_standard_commands(&host);
        host
    }

    /// Every operation is `Pending` once before it completes.
    pub fn set_latency(&self, on: bool) {
        self.latency.set(on);
    }

    /// The separator of `PATH` entries (`path_entries`).
    pub fn set_path_sep(&self, sep: u8) {
        self.path_sep.set(sep);
    }

    /// Act as a Windows host for line ends (`Host::strips_crlf`).
    pub fn set_strips_crlf(&self, on: bool) {
        self.strips_crlf.set(on);
    }

    pub fn set_stdin(&self, data: &[u8]) {
        *self.stdin.borrow_mut() = data.to_vec();
    }

    /// Adds a fake command, found on any `PATH` as `/bin/NAME`.
    pub fn add_command(
        &self,
        name: &str,
        f: impl Fn(FakeProc) -> Pin<Box<dyn Future<Output = i32>>> + 'static,
    ) {
        self.commands
            .borrow_mut()
            .insert(name.as_bytes().to_vec(), Rc::new(f));
    }

    pub fn stdout(&self) -> Vec<u8> {
        self.stdout.borrow().clone()
    }

    pub fn stderr(&self) -> Vec<u8> {
        self.stderr.borrow().clone()
    }

    pub fn live_procs(&self) -> usize {
        self.counters.procs.get()
    }

    pub fn open_files(&self) -> usize {
        self.counters.files.get()
    }

    pub fn live_pipes(&self) -> usize {
        self.counters.pipes.get()
    }

    fn mem_file(
        &self,
        data: Rc<RefCell<Vec<u8>>>,
        read: bool,
        write: bool,
        append: bool,
    ) -> Rc<dyn HostFile> {
        self.counters.files.set(self.counters.files.get() + 1);
        Rc::new(MemFile {
            data,
            pos: Cell::new(0),
            read,
            write,
            append,
            toggle: Toggle::new(self.latency.get()),
            counters: self.counters.clone(),
        })
    }

    /// A shell config for this host: its stdio, `/tmp` as the cwd, a small env.
    pub fn config(self: &Rc<Self>) -> crate::ShellConfig {
        let mut cfg = crate::ShellConfig::new(self.clone());
        cfg.stdin = Some(self.mem_file(self.stdin.clone(), true, false, false));
        cfg.stdout = Some(self.mem_file(self.stdout.clone(), false, true, true));
        cfg.stderr = Some(self.mem_file(self.stderr.clone(), false, true, true));
        cfg.cwd = b"/tmp".to_vec();
        cfg.env = vec![
            (b"PATH".to_vec(), b"/bin".to_vec()),
            (b"HOME".to_vec(), b"/tmp".to_vec()),
        ];
        cfg
    }

    fn pipe_pair(&self) -> (TPipe, TPipe) {
        self.counters.pipes.set(self.counters.pipes.get() + 1);
        let inner = Rc::new(RefCell::new(TPipeInner {
            buf: VecDeque::new(),
            reader: true,
            writer: true,
            waker: None,
            wwaker: None,
        }));
        let mk = |read_end| TPipe {
            inner: inner.clone(),
            read_end,
            toggle: Toggle::new(self.latency.get()),
            counters: self.counters.clone(),
        };
        (mk(true), mk(false))
    }
}

impl Host for TestHost {
    fn open(&self, path: &[u8], opts: OpenOptions) -> HostFuture<io::Result<Rc<dyn HostFile>>> {
        let r = if path == b"/dev/null" {
            Ok(Rc::new(NullFile) as Rc<dyn HostFile>)
        } else {
            self.fs
                .open(path, opts)
                .map(|data| self.mem_file(data, opts.read, opts.write, opts.append))
        };
        delayed(self.latency.get(), r)
    }

    fn stat(&self, path: &[u8], follow: bool) -> HostFuture<io::Result<Metadata>> {
        delayed(self.latency.get(), self.fs.stat(path, follow))
    }

    fn stat_extra(
        &self,
        path: &[u8],
        follow: bool,
    ) -> HostFuture<io::Result<(Metadata, ExtraMetadata)>> {
        delayed(self.latency.get(), self.fs.stat_extra(path, follow))
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        delayed(self.latency.get(), self.fs.mkdir(path, mode))
    }

    fn remove_file(&self, path: &[u8]) -> HostFuture<io::Result<()>> {
        delayed(self.latency.get(), self.fs.remove(path, false))
    }

    fn remove_dir(&self, path: &[u8]) -> HostFuture<io::Result<()>> {
        delayed(self.latency.get(), self.fs.remove(path, true))
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> HostFuture<io::Result<()>> {
        delayed(self.latency.get(), self.fs.rename(from, to))
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        delayed(self.latency.get(), self.fs.symlink_at(target, link))
    }

    fn hard_link(&self, existing: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        delayed(self.latency.get(), self.fs.hard_link(existing, link))
    }

    fn read_link(&self, path: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        delayed(self.latency.get(), self.fs.read_link(path))
    }

    fn set_mode(&self, path: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        delayed(self.latency.get(), self.fs.set_mode(path, mode))
    }

    fn set_times(
        &self,
        path: &[u8],
        atime_ns: Option<i64>,
        mtime_ns: Option<i64>,
        follow: bool,
    ) -> HostFuture<io::Result<()>> {
        delayed(
            self.latency.get(),
            self.fs.set_times(path, atime_ns, mtime_ns, follow),
        )
    }

    fn user_name(&self, uid: u32) -> Option<Vec<u8>> {
        (uid == 1000).then(|| b"user".to_vec())
    }

    fn group_name(&self, gid: u32) -> Option<Vec<u8>> {
        (gid == 1000).then(|| b"users".to_vec())
    }

    fn access(&self, path: &[u8], mode: Access) -> HostFuture<io::Result<bool>> {
        let r = self.fs.stat(path, true).map(|m| match mode {
            Access::Read => m.mode & 0o444 != 0,
            Access::Write => m.mode & 0o222 != 0,
            Access::Execute => m.mode & 0o111 != 0,
        });
        delayed(self.latency.get(), r)
    }

    fn read_dir(&self, path: &[u8]) -> HostFuture<io::Result<Vec<DirEntry>>> {
        delayed(self.latency.get(), self.fs.read_dir(path))
    }

    fn canonicalize(&self, path: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        delayed(self.latency.get(), self.fs.canonicalize(path))
    }

    fn strips_crlf(&self) -> bool {
        self.strips_crlf.get()
    }

    /// A few fixed limits, so `ulimit` has something to report; the rest are absent.
    fn rlimit(
        &self,
        resource: crate::host::Rlimit,
    ) -> Option<(crate::host::Limit, crate::host::Limit)> {
        use crate::host::Rlimit;
        match resource {
            Rlimit::NoFile => Some((Some(1024), Some(4096))),
            Rlimit::Core => Some((Some(0), None)),
            Rlimit::FileSize | Rlimit::Cpu => Some((None, None)),
            Rlimit::Stack => Some((Some(8 << 20), None)),
            _ => None,
        }
    }

    fn path_entries(&self, path: &[u8]) -> Vec<Vec<u8>> {
        let sep = self.path_sep.get();
        path.split(|&b| b == sep).map(<[u8]>::to_vec).collect()
    }

    fn resolve(&self, name: &[u8], _path: &[u8], _cwd: &[u8]) -> HostFuture<Option<Vec<u8>>> {
        let found = self.commands.borrow().contains_key(name).then(|| {
            let mut p = b"/bin/".to_vec();
            p.extend_from_slice(name);
            p
        });
        delayed(self.latency.get(), found)
    }

    fn spawn(&self, spec: SpawnSpec) -> io::Result<Spawned> {
        // A file of the in-memory fs is a script for the shell to run (D1300).
        if let Ok(p) = std::str::from_utf8(&spec.program)
            && self.fs.read_file(p).is_some()
        {
            return Err(crate::host::RunInShell::error());
        }
        let name = spec
            .program
            .strip_prefix(b"/bin/")
            .ok_or(io::ErrorKind::NotFound)?;
        let cmd = self
            .commands
            .borrow()
            .get(name)
            .cloned()
            .ok_or(io::ErrorKind::NotFound)?;
        let mut stdio: [Option<Rc<dyn HostFile>>; 3] = [None, None, None];
        let mut pipes = Vec::new();
        let mut given: Vec<(u32, Rc<dyn HostFile>)> = Vec::new();
        for (n, fd) in spec.fds {
            let theirs: Rc<dyn HostFile> = match fd {
                ChildFd::File(f) => f,
                ChildFd::Dup(m) => given
                    .iter()
                    .find(|(k, _)| *k == m)
                    .map(|(_, f)| f.clone())
                    .ok_or(io::ErrorKind::InvalidInput)?,
                ChildFd::Null => Rc::new(NullFile),
                ChildFd::Pipe { input } => {
                    let (r, w) = self.pipe_pair();
                    if input {
                        pipes.push((n, Rc::new(w) as Rc<dyn HostFile>));
                        Rc::new(r)
                    } else {
                        pipes.push((n, Rc::new(r) as Rc<dyn HostFile>));
                        Rc::new(w)
                    }
                }
            };
            given.push((n, theirs.clone()));
            if (n as usize) < 3 {
                stdio[n as usize] = Some(theirs);
            }
        }
        let [i, o, e] = stdio;
        let null = || Rc::new(NullFile) as Rc<dyn HostFile>;
        let proc = FakeProc {
            argv: spec.argv,
            env: spec.env,
            cwd: spec.cwd,
            stdin: i.unwrap_or_else(null),
            stdout: o.unwrap_or_else(null),
            stderr: e.unwrap_or_else(null),
            fs: self.fs.clone(),
        };
        self.counters.procs.set(self.counters.procs.get() + 1);
        self.counters.spawned.set(self.counters.spawned.get() + 1);
        Ok(Spawned {
            proc: Box::new(FakeExt {
                pid: 5000 + self.counters.spawned.get(),
                waker: None,
                fut: Some(cmd(proc)),
                status: None,
                delay: Delay {
                    on: self.latency.get(),
                },
                counters: self.counters.clone(),
            }),
            pipes,
        })
    }

    /// A fixed instant (2023-11-14T22:13:20Z): tests stay deterministic.
    fn now(&self) -> Duration {
        Duration::from_secs(1_700_000_000)
    }

    fn sleep(&self, _duration: Duration) -> HostFuture<()> {
        // Virtual time: a sleep is over at once (after one `Pending` in latency mode).
        delayed(self.latency.get(), ())
    }

    fn pid(&self) -> u32 {
        4242
    }

    fn home_dir(&self, user: &[u8]) -> Option<Vec<u8>> {
        (user == b"root").then(|| b"/root".to_vec())
    }
}

fn arg_num(p: &FakeProc, flag: &[u8], default: usize) -> usize {
    p.argv
        .windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| std::str::from_utf8(&w[1]).ok()?.parse().ok())
        .unwrap_or(default)
}

/// `cat [FILE…]`, `yes`, `head -n N`, `wc -l`, `argv` (one `[arg]` per line), `env`,
/// `true`, `false`, `hang` (never exits), `exit N`.
fn add_standard_commands(host: &TestHost) {
    host.add_command("cat", |p| {
        Box::pin(async move {
            if p.argv.len() > 1 {
                for a in &p.argv[1..] {
                    let mut path = p.cwd.clone();
                    path.push(b'/');
                    path.extend_from_slice(a);
                    let path = if a.starts_with(b"/") { a.clone() } else { path };
                    match p.fs.read_file(&String::from_utf8_lossy(&path)) {
                        Some(d) => {
                            if p.write(&d).await.is_err() {
                                return 1;
                            }
                        }
                        None => {
                            let _ = p.write_err(b"cat: no such file\n").await;
                            return 1;
                        }
                    }
                }
                return 0;
            }
            let mut buf = [0u8; 1000];
            loop {
                let n = match poll_fn(|cx| p.stdin.poll_read(cx, &mut buf)).await {
                    Ok(0) | Err(_) => return 0,
                    Ok(n) => n,
                };
                if p.write(&buf[..n]).await.is_err() {
                    return 141;
                }
            }
        })
    });
    host.add_command("yes", |p| {
        Box::pin(async move {
            loop {
                if p.write(b"y\n").await.is_err() {
                    return 141;
                }
            }
        })
    });
    host.add_command("head", |p| {
        Box::pin(async move {
            let n = arg_num(&p, b"-n", 10);
            for _ in 0..n {
                let line = p.read_line().await;
                if line.is_empty() {
                    break;
                }
                if p.write(&line).await.is_err() {
                    return 1;
                }
            }
            0
        })
    });
    host.add_command("wc", |p| {
        Box::pin(async move {
            let data = p.read_all().await;
            let n = data.iter().filter(|&&b| b == b'\n').count();
            let _ = p.write(format!("{n}\n").as_bytes()).await;
            0
        })
    });
    host.add_command("argv", |p| {
        Box::pin(async move {
            let mut out = Vec::new();
            for a in &p.argv[1..] {
                out.push(b'[');
                out.extend_from_slice(a);
                out.extend_from_slice(b"]\n");
            }
            let _ = p.write(&out).await;
            0
        })
    });
    host.add_command("env", |p| {
        Box::pin(async move {
            let mut out = Vec::new();
            for (k, v) in &p.env {
                out.extend_from_slice(k);
                out.push(b'=');
                out.extend_from_slice(v);
                out.push(b'\n');
            }
            let _ = p.write(&out).await;
            0
        })
    });
    host.add_command("true", |_| Box::pin(async { 0 }));
    host.add_command("false", |_| Box::pin(async { 1 }));
    // Keeps its descriptors open, as a real process that hangs does.
    host.add_command("hang", |p| {
        Box::pin(async move {
            let _fds = p;
            std::future::pending::<i32>().await
        })
    });
    host.add_command("exit", |p| {
        Box::pin(async move {
            p.argv
                .get(1)
                .and_then(|a| std::str::from_utf8(a).ok()?.parse().ok())
                .unwrap_or(0)
        })
    });
}
