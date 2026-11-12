//! File descriptors of a virtual process: what each number refers to, and reading and
//! writing them without blocking the thread.

use std::collections::BTreeMap;
use std::future::poll_fn;
use std::io;
use std::rc::Rc;
use std::task::{Context, Poll};

use crate::host::HostFile;
use crate::pipe::{PipeReader, PipeWriter};

/// What a descriptor refers to. Shared by `Rc` between descriptors that are
/// duplicates (`2>&1`) and between a vproc and its children; the last holder closes it.
pub(crate) enum FdObj {
    PipeRead(PipeReader),
    PipeWrite(PipeWriter),
    File(Rc<dyn HostFile>),
}

pub(crate) type Fd = Rc<FdObj>;

/// EBADF, without OS error numbers (wasm has none).
pub(crate) fn bad_fd() -> io::Error {
    io::Error::other("Bad file descriptor")
}

impl FdObj {
    pub(crate) fn poll_read(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self {
            FdObj::PipeRead(r) => r.poll_read(cx, buf),
            FdObj::PipeWrite(_) => Poll::Ready(Err(bad_fd())),
            FdObj::File(f) => f.poll_read(cx, buf),
        }
    }

    pub(crate) fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self {
            FdObj::PipeWrite(w) => w.poll_write(cx, buf),
            FdObj::PipeRead(_) => Poll::Ready(Err(bad_fd())),
            FdObj::File(f) => f.poll_write(cx, buf),
        }
    }

    pub(crate) async fn write_all(&self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let n = poll_fn(|cx| self.poll_write(cx, data)).await?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            data = &data[n..];
        }
        Ok(())
    }

    pub(crate) async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        poll_fn(|cx| self.poll_read(cx, buf)).await
    }

    pub(crate) async fn read_to_end(&self, out: &mut Vec<u8>) -> io::Result<()> {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = self.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            out.extend_from_slice(&buf[..n]);
        }
    }
}

/// Copies `from` to `to` until end of file or an error on either side: between an
/// external process's pipe and an engine object. Polled by whoever waits for the
/// external, so dropping the wait drops the pump and both ends.
pub(crate) struct Pump {
    from: Fd,
    to: Fd,
    buf: Box<[u8]>,
    start: usize,
    end: usize,
    done: bool,
}

impl Pump {
    pub(crate) fn new(from: Fd, to: Fd) -> Pump {
        Pump {
            from,
            to,
            buf: vec![0u8; 16 * 1024].into_boxed_slice(),
            start: 0,
            end: 0,
            done: false,
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.done
    }

    /// Moves data until one side has to wait (`Pending`) or the pump ends.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        while !self.done {
            if self.start < self.end {
                match self.to.poll_write(cx, &self.buf[self.start..self.end]) {
                    Poll::Ready(Ok(n)) if n > 0 => self.start += n,
                    // A write error ends the pump; dropping `from` then closes the
                    // external's pipe, which gets SIGPIPE on its next write, as with
                    // a closed OS pipe.
                    Poll::Ready(_) => self.done = true,
                    Poll::Pending => return Poll::Pending,
                }
            } else {
                match self.from.poll_read(cx, &mut self.buf) {
                    Poll::Ready(Ok(0)) | Poll::Ready(Err(_)) => self.done = true,
                    Poll::Ready(Ok(n)) => {
                        self.start = 0;
                        self.end = n;
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }
        }
        Poll::Ready(())
    }
}

/// A vproc's descriptor table.
#[derive(Clone, Default)]
pub(crate) struct FdTable {
    fds: BTreeMap<u32, Fd>,
}

impl FdTable {
    pub(crate) fn get(&self, n: u32) -> Option<&Fd> {
        self.fds.get(&n)
    }

    pub(crate) fn set(&mut self, n: u32, fd: Fd) {
        self.fds.insert(n, fd);
    }

    pub(crate) fn close(&mut self, n: u32) {
        self.fds.remove(&n);
    }

    /// Whether every descriptor is a file of the host: a forked copy of the process
    /// shares those with it, but not a pipe between two vprocs of this engine.
    pub(crate) fn all_files(&self) -> bool {
        self.fds.values().all(|f| matches!(**f, FdObj::File(_)))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, &Fd)> {
        self.fds.iter().map(|(n, f)| (*n, f))
    }
}
