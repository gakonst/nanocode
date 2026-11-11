//! In-memory pipes between virtual processes (ARCH.md "Pipes between vprocs").
//!
//! A bounded buffer with reader and writer counts. Reading an empty pipe waits while a
//! writer exists; writing a full pipe waits while a reader exists; writing without
//! readers is `BrokenPipe` (the vproc's SIGPIPE). Both sides are tasks of the same
//! scheduler, so waiting never blocks a thread.
//!
//! **Materialized pipes (D1068).** When an external is started on an end of a pipe
//! whose buffer is empty, the pipe becomes an OS pipe of the host (`Host::pipe`): the
//! external gets the OS end itself, so `a | b` of two externals moves no byte through
//! the engine. From then on vproc writes go to the OS write end (the buffer was empty,
//! so nothing overtakes), and vproc reads take what is left in the buffer, then read the
//! OS read end directly. The engine holds each OS end while it
//! holds a reader or writer of that side; the kernel counts the externals' copies.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use crate::host::{Host, HostFile};

/// The capacity of a pipe, as on Linux.
pub(crate) const PIPE_CAPACITY: usize = 64 * 1024;

struct Inner {
    buf: VecDeque<u8>,
    readers: usize,
    writers: usize,
    read_wakers: Vec<Waker>,
    write_wakers: Vec<Waker>,
    /// The OS pipe once materialized: (read end, write end), each dropped when the
    /// engine's last reader or writer goes.
    os: Option<(OsEnd, OsEnd)>,
}

type OsEnd = Option<Rc<dyn HostFile>>;

fn register(list: &mut Vec<Waker>, w: &Waker) {
    if !list.iter().any(|x| x.will_wake(w)) {
        list.push(w.clone());
    }
}

fn wake_all(list: &mut Vec<Waker>) {
    for w in list.drain(..) {
        w.wake();
    }
}

pub(crate) struct PipeReader(Rc<RefCell<Inner>>);
pub(crate) struct PipeWriter(Rc<RefCell<Inner>>);

pub(crate) fn pipe() -> (PipeReader, PipeWriter) {
    let inner = Rc::new(RefCell::new(Inner {
        buf: VecDeque::new(),
        readers: 1,
        writers: 1,
        read_wakers: Vec::new(),
        write_wakers: Vec::new(),
        os: None,
    }));
    (PipeReader(inner.clone()), PipeWriter(inner))
}

/// A pipe that already holds `data` and has no writer: here-documents and
/// here-strings. The body is in memory anyway, so nothing is written before the
/// consumer runs (pitfall 2 of `research/execution-core.md`).
pub(crate) fn preloaded(data: &[u8]) -> PipeReader {
    PipeReader(Rc::new(RefCell::new(Inner {
        buf: data.to_vec().into(),
        readers: 1,
        writers: 0,
        read_wakers: Vec::new(),
        write_wakers: Vec::new(),
        os: None,
    })))
}

/// The OS end for an external started on a pipe end: `reading` = the read end.
/// Materializes the pipe if its buffer is empty and the host has OS pipes; `None` means
/// the external needs a pump (buffered bytes come first, or no OS pipes).
fn os_end(inner: &Rc<RefCell<Inner>>, reading: bool, host: &dyn Host) -> Option<Rc<dyn HostFile>> {
    let mut p = inner.borrow_mut();
    if !p.buf.is_empty() {
        return None;
    }
    if p.os.is_none() {
        let (r, w) = host.pipe().ok()?;
        let r = (p.readers > 0).then_some(r);
        let w = (p.writers > 0).then_some(w);
        p.os = Some((r, w));
        // Waiting vprocs poll the OS ends from now on.
        wake_all(&mut p.read_wakers);
        wake_all(&mut p.write_wakers);
    }
    let (r, w) = p.os.as_ref()?;
    if reading { r.clone() } else { w.clone() }
}

impl PipeReader {
    /// See [`os_end`].
    pub(crate) fn os_end(&self, host: &dyn Host) -> Option<Rc<dyn HostFile>> {
        os_end(&self.0, true, host)
    }

    pub(crate) fn poll_read(
        &self,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut p = self.0.borrow_mut();
        if out.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if p.buf.is_empty()
            && let Some((Some(r), _)) = &p.os
        {
            // Nothing buffered: read the OS pipe straight into the caller's buffer
            // (no copy, no read-ahead to hold for a later pump).
            let r = r.clone();
            drop(p);
            return r.poll_read(cx, out);
        }
        if !p.buf.is_empty() {
            let n = out.len().min(p.buf.len());
            // The two runs of the ring, copied as slices rather than byte by byte.
            let (front, back) = p.buf.as_slices();
            let first = n.min(front.len());
            out[..first].copy_from_slice(&front[..first]);
            out[first..n].copy_from_slice(&back[..n - first]);
            p.buf.drain(..n);
            wake_all(&mut p.write_wakers);
            return Poll::Ready(Ok(n));
        }
        if p.writers == 0 {
            return Poll::Ready(Ok(0));
        }
        register(&mut p.read_wakers, cx.waker());
        Poll::Pending
    }
}

impl PipeWriter {
    /// See [`os_end`].
    pub(crate) fn os_end(&self, host: &dyn Host) -> Option<Rc<dyn HostFile>> {
        os_end(&self.0, false, host)
    }

    pub(crate) fn poll_write(&self, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let p = self.0.borrow();
        if let Some((_, Some(w))) = &p.os {
            let w = w.clone();
            drop(p);
            return w.poll_write(cx, data);
        }
        drop(p);
        let mut p = self.0.borrow_mut();
        if p.readers == 0 {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let space = PIPE_CAPACITY.saturating_sub(p.buf.len());
        if space == 0 {
            register(&mut p.write_wakers, cx.waker());
            return Poll::Pending;
        }
        let n = space.min(data.len());
        p.buf.extend(&data[..n]);
        wake_all(&mut p.read_wakers);
        Poll::Ready(Ok(n))
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        let mut p = self.0.borrow_mut();
        p.readers -= 1;
        if p.readers == 0 {
            p.buf.clear();
            if let Some((r, _)) = &mut p.os {
                *r = None;
            }
            wake_all(&mut p.write_wakers);
        }
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        let mut p = self.0.borrow_mut();
        p.writers -= 1;
        if p.writers == 0 {
            if let Some((_, w)) = &mut p.os {
                *w = None;
            }
            wake_all(&mut p.read_wakers);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_write_eof_and_broken_pipe() {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let (r, w) = pipe();
        let mut buf = [0u8; 4];
        assert!(r.poll_read(&mut cx, &mut buf).is_pending());
        assert!(matches!(
            w.poll_write(&mut cx, b"hello"),
            Poll::Ready(Ok(5))
        ));
        assert!(matches!(r.poll_read(&mut cx, &mut buf), Poll::Ready(Ok(4))));
        assert_eq!(&buf, b"hell");
        drop(w);
        assert!(matches!(r.poll_read(&mut cx, &mut buf), Poll::Ready(Ok(1))));
        assert!(matches!(r.poll_read(&mut cx, &mut buf), Poll::Ready(Ok(0))));

        let (r, w) = pipe();
        drop(r);
        assert!(
            matches!(w.poll_write(&mut cx, b"x"), Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::BrokenPipe)
        );
    }

    #[test]
    fn full_pipe_waits() {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let (_r, w) = pipe();
        let big = vec![0u8; PIPE_CAPACITY + 10];
        assert!(matches!(w.poll_write(&mut cx, &big), Poll::Ready(Ok(n)) if n == PIPE_CAPACITY));
        assert!(w.poll_write(&mut cx, b"x").is_pending());
    }
}
