use crate::{
    fs::{Reply, Write},
    host::Vfs,
    machine::Machine,
};
use bytes::Bytes;
use psh_engine::{
    HostFile,
    host::{FileKind, OpenOptions},
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

pub(crate) const PIPE_BYTES: usize = 65_536;
const FILE_MODE: u32 = 0o644;

pub(crate) fn bad_descriptor() -> io::Error {
    io::Error::other("Bad file descriptor")
}

pub(crate) struct Source {
    data: RefCell<Bytes>,
    at: Cell<usize>,
}

impl Source {
    pub(crate) const fn new(data: Bytes) -> Self {
        Self {
            data: RefCell::new(data),
            at: Cell::new(0),
        }
    }

    pub(crate) fn set(&self, data: Bytes) {
        *self.data.borrow_mut() = data;
        self.at.set(0);
    }
}

impl HostFile for Source {
    fn poll_read(&self, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let data = self.data.borrow();
        let at = self.at.get();
        let count = buf.len().min(data.len() - at);
        buf[..count].copy_from_slice(&data[at..at + count]);
        self.at.set(at + count);
        Poll::Ready(Ok(count))
    }

    fn poll_write(&self, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(bad_descriptor()))
    }

    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::File)
    }
}

pub(crate) struct Capture {
    data: RefCell<Vec<u8>>,
    limit: u64,
    truncated: Cell<bool>,
}

impl Capture {
    pub(crate) const fn new(limit: u64) -> Self {
        Self {
            data: RefCell::new(Vec::new()),
            limit,
            truncated: Cell::new(false),
        }
    }

    pub(crate) fn take(&self) -> (Vec<u8>, bool) {
        (self.data.take(), self.truncated.replace(false))
    }
}

impl HostFile for Capture {
    fn poll_read(&self, _cx: &mut Context<'_>, _buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(bad_descriptor()))
    }

    fn poll_write(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut data = self.data.borrow_mut();
        let room = self.limit.saturating_sub(data.len() as u64);
        let keep = buf.len().min(usize::try_from(room).unwrap_or(usize::MAX));
        data.extend_from_slice(&buf[..keep]);
        if keep < buf.len() {
            self.truncated.set(true);
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::Fifo)
    }
}

pub(crate) enum Device {
    Null,
    Zero,
    Random(Rc<dyn Machine>),
}

impl Device {
    pub(crate) fn at(path: &[u8], machine: &Rc<dyn Machine>) -> Option<Self> {
        match path {
            b"/dev/null" => Some(Self::Null),
            b"/dev/zero" => Some(Self::Zero),
            b"/dev/random" | b"/dev/urandom" => Some(Self::Random(machine.clone())),
            _ => None,
        }
    }

    pub(crate) const NAMES: [&'static [u8]; 4] = [b"null", b"random", b"urandom", b"zero"];
}

impl HostFile for Device {
    fn poll_read(&self, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        match self {
            Self::Null => Poll::Ready(Ok(0)),
            Self::Zero => {
                buf.fill(0);
                Poll::Ready(Ok(buf.len()))
            }
            Self::Random(machine) => {
                machine.random(buf);
                Poll::Ready(Ok(buf.len()))
            }
        }
    }

    fn poll_write(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::CharDevice)
    }
}

struct Reader {
    data: Bytes,
    at: Cell<usize>,
}

impl HostFile for Reader {
    fn poll_read(&self, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let at = self.at.get();
        let count = buf.len().min(self.data.len() - at);
        buf[..count].copy_from_slice(&self.data[at..at + count]);
        self.at.set(at + count);
        Poll::Ready(Ok(count))
    }

    fn poll_write(&self, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(bad_descriptor()))
    }

    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::File)
    }
}

type Pending = Pin<Box<dyn Future<Output = io::Result<()>>>>;

struct Appender {
    vfs: Rc<Vfs>,
    path: Vec<u8>,
    size: Cell<u64>,
    pending: RefCell<Option<(Pending, usize)>>,
}

impl HostFile for Appender {
    fn poll_read(&self, _cx: &mut Context<'_>, _buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(bad_descriptor()))
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut pending = self.pending.borrow_mut();
        if pending.is_none() {
            let size = self.size.get() + buf.len() as u64;
            if size > self.vfs.limits.file {
                return Poll::Ready(Err(io::ErrorKind::FileTooLarge.into()));
            }
            self.size.set(size);
            match self.vfs.write(&self.path, buf, Write::Append) {
                Reply::Now(result) => return Poll::Ready(result.map(|()| buf.len())),
                Reply::Later(write) => *pending = Some((write, buf.len())),
            }
        }
        let Some((write, count)) = pending.as_mut() else {
            return Poll::Ready(Err(bad_descriptor()));
        };
        let count = *count;
        match write.as_mut().poll(cx) {
            Poll::Ready(result) => {
                *pending = None;
                Poll::Ready(result.map(|()| count))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::File)
    }
}

struct Updater {
    vfs: Rc<Vfs>,
    path: Vec<u8>,
    data: RefCell<Vec<u8>>,
    at: Cell<usize>,
    append: bool,
    readable: bool,
    pending: RefCell<Option<(Pending, usize)>>,
}

impl HostFile for Updater {
    fn poll_read(&self, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if !self.readable {
            return Poll::Ready(Err(bad_descriptor()));
        }
        let data = self.data.borrow();
        let at = self.at.get().min(data.len());
        let count = buf.len().min(data.len() - at);
        buf[..count].copy_from_slice(&data[at..at + count]);
        self.at.set(at + count);
        Poll::Ready(Ok(count))
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut pending = self.pending.borrow_mut();
        if pending.is_none() {
            let mut data = self.data.borrow_mut();
            let at = if self.append {
                data.len()
            } else {
                self.at.get()
            };
            let end = at + buf.len();
            if end as u64 > self.vfs.limits.file {
                return Poll::Ready(Err(io::ErrorKind::FileTooLarge.into()));
            }
            if data.len() < end {
                data.resize(end, 0);
            }
            data[at..end].copy_from_slice(buf);
            self.at.set(end);
            match self.vfs.write(&self.path, &data, Write::Replace) {
                Reply::Now(result) => return Poll::Ready(result.map(|()| buf.len())),
                Reply::Later(write) => *pending = Some((write, buf.len())),
            }
        }
        let Some((write, count)) = pending.as_mut() else {
            return Poll::Ready(Err(bad_descriptor()));
        };
        let count = *count;
        match write.as_mut().poll(cx) {
            Poll::Ready(result) => {
                *pending = None;
                Poll::Ready(result.map(|()| count))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::File)
    }
}

pub(crate) async fn open(
    vfs: Rc<Vfs>,
    path: Vec<u8>,
    options: OpenOptions,
) -> io::Result<Rc<dyn HostFile>> {
    if !options.write && !options.append {
        let data = vfs.read(&path).get().await?;
        return Ok(Rc::new(Reader {
            data,
            at: Cell::new(0),
        }));
    }
    let sequential = !options.read && (options.truncate || options.append || options.create_new);
    let mode = options.mode.filter(|&mode| mode & 0o7777 != FILE_MODE);
    let known = !options.create || mode.is_some() || !sequential;
    let existing = if known {
        match vfs.stat(&path, true).get().await {
            Ok(stat) => Some(stat),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    if known && existing.is_none() && !options.create {
        return Err(io::ErrorKind::NotFound.into());
    }
    if existing.is_some() && options.create_new {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    let created = known && existing.is_none();
    let file: Rc<dyn HostFile> = if sequential {
        let how = if options.create_new {
            Write::New
        } else if options.truncate {
            Write::Replace
        } else {
            Write::Append
        };
        vfs.write(&path, &[], how).get().await?;
        let size = match existing {
            Some(stat) if !options.truncate => stat.size,
            _ => 0,
        };
        Rc::new(Appender {
            vfs: vfs.clone(),
            path: path.clone(),
            size: Cell::new(size),
            pending: RefCell::new(None),
        })
    } else {
        let data = if existing.is_some() && !options.truncate {
            vfs.read(&path).get().await?.to_vec()
        } else {
            vfs.write(&path, &[], Write::Replace).get().await?;
            Vec::new()
        };
        Rc::new(Updater {
            vfs: vfs.clone(),
            path: path.clone(),
            data: RefCell::new(data),
            at: Cell::new(0),
            append: options.append,
            readable: options.read,
            pending: RefCell::new(None),
        })
    };
    if let (true, Some(mode)) = (created, mode) {
        match vfs.set_mode(&path, mode & 0o7777).get().await {
            Err(error) if error.kind() != io::ErrorKind::Unsupported => return Err(error),
            _ => {}
        }
    }
    Ok(file)
}

struct Channel {
    data: VecDeque<u8>,
    reader: bool,
    writer: bool,
    reading: Option<Waker>,
    writing: Option<Waker>,
}

pub(crate) struct PipeEnd {
    channel: Rc<RefCell<Channel>>,
    reads: bool,
}

pub(crate) fn pipe() -> (PipeEnd, PipeEnd) {
    let channel = Rc::new(RefCell::new(Channel {
        data: VecDeque::new(),
        reader: true,
        writer: true,
        reading: None,
        writing: None,
    }));
    (
        PipeEnd {
            channel: channel.clone(),
            reads: true,
        },
        PipeEnd {
            channel,
            reads: false,
        },
    )
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        let mut channel = self.channel.borrow_mut();
        let waker = if self.reads {
            channel.reader = false;
            channel.writing.take()
        } else {
            channel.writer = false;
            channel.reading.take()
        };
        drop(channel);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl HostFile for PipeEnd {
    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if !self.reads {
            return Poll::Ready(Err(bad_descriptor()));
        }
        let mut channel = self.channel.borrow_mut();
        if channel.data.is_empty() {
            if !channel.writer {
                return Poll::Ready(Ok(0));
            }
            channel.reading = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let count = buf.len().min(channel.data.len());
        let (front, back) = channel.data.as_slices();
        let first = count.min(front.len());
        buf[..first].copy_from_slice(&front[..first]);
        buf[first..count].copy_from_slice(&back[..count - first]);
        channel.data.drain(..count);
        let waker = channel.writing.take();
        drop(channel);
        if let Some(waker) = waker {
            waker.wake();
        }
        Poll::Ready(Ok(count))
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.reads {
            return Poll::Ready(Err(bad_descriptor()));
        }
        let mut channel = self.channel.borrow_mut();
        if !channel.reader {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let room = PIPE_BYTES.saturating_sub(channel.data.len());
        if room == 0 {
            channel.writing = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let count = room.min(buf.len());
        channel.data.extend(&buf[..count]);
        let waker = channel.reading.take();
        drop(channel);
        if let Some(waker) = waker {
            waker.wake();
        }
        Poll::Ready(Ok(count))
    }

    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::Fifo)
    }
}
