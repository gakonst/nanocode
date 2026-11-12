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
//!
//! **A pipe that holds bytes (nanocode).** An external started on the read end of a
//! pipe whose buffer is not empty gets the OS read end too, as a child of bash gets the
//! pipe itself: it takes only what it reads, and the rest stays for the next reader
//! (the next `read` of a loop whose body ran the external). The buffered bytes become
//! the backlog: they go into the OS pipe in order, as far as it has room, whenever a
//! vproc reads or writes the pipe and while an external reads it (a [`Feed`], polled by
//! whoever waits for the external). While a backlog is left, vproc writes join it, so
//! nothing overtakes, and an external writer gets a pump. The engine keeps the OS write
//! end until the backlog is gone.

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
    /// Once materialized: what is still to go into the OS pipe, behind what it holds.
    backlog: VecDeque<u8>,
    /// The [`Feed`]s of externals reading the OS read end.
    feeds: usize,
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
        backlog: VecDeque::new(),
        feeds: 0,
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
        backlog: VecDeque::new(),
        feeds: 0,
        readers: 1,
        writers: 0,
        read_wakers: Vec::new(),
        write_wakers: Vec::new(),
        os: None,
    })))
}

/// The OS end for an external started on a pipe end: `reading` = the read end.
/// Materializes the pipe if the host has OS pipes: for a reader what the buffer holds
/// becomes the backlog; for a writer only when the buffer is empty. `None` means the
/// external needs a pump (its bytes must come behind buffered ones, bytes a `read` gave
/// back come before the OS pipe's, or no OS pipes).
fn os_end(inner: &Rc<RefCell<Inner>>, reading: bool, host: &dyn Host) -> Option<Rc<dyn HostFile>> {
    let mut p = inner.borrow_mut();
    if p.os.is_none() {
        if !reading && !p.buf.is_empty() {
            return None;
        }
        let (r, w) = host.pipe().ok()?;
        p.backlog = std::mem::take(&mut p.buf);
        let r = (p.readers > 0).then_some(r);
        let w = (p.writers > 0 || !p.backlog.is_empty()).then_some(w);
        p.os = Some((r, w));
        // Waiting vprocs poll the OS ends from now on.
        wake_all(&mut p.read_wakers);
        wake_all(&mut p.write_wakers);
    }
    if if reading {
        !p.buf.is_empty()
    } else {
        !p.backlog.is_empty()
    } {
        return None;
    }
    let (r, w) = p.os.as_ref()?;
    if reading { r.clone() } else { w.clone() }
}

/// Moves the backlog into the OS pipe as far as it has room. Once it is gone, a pipe
/// without writers lets go of the OS write end, so its readers see the end.
fn flush(inner: &Rc<RefCell<Inner>>, cx: &mut Context<'_>) {
    loop {
        let wrote = {
            let p = inner.borrow();
            if p.backlog.is_empty() {
                return;
            }
            let Some((_, Some(w))) = &p.os else { return };
            w.poll_write(cx, p.backlog.as_slices().0)
        };
        let mut p = inner.borrow_mut();
        match wrote {
            Poll::Pending => return,
            Poll::Ready(Ok(n)) if n > 0 => {
                p.backlog.drain(..n);
                wake_all(&mut p.write_wakers);
            }
            // Nobody reads the OS pipe any more.
            Poll::Ready(_) => p.backlog.clear(),
        }
        if p.backlog.is_empty() {
            release(&mut p);
            return;
        }
    }
}

/// The OS write end goes once no writer and no backlog needs it.
fn release(p: &mut Inner) {
    if p.writers == 0
        && p.backlog.is_empty()
        && let Some((_, w)) = &mut p.os
    {
        *w = None;
    }
}

/// The backlog has no reader left: no vproc holds the read end and no external's wait
/// moves it.
fn abandon(p: &mut Inner) {
    if p.readers == 0 && p.feeds == 0 {
        p.backlog.clear();
        release(p);
        wake_all(&mut p.write_wakers);
    }
}

/// Moves a materialized pipe's backlog into its OS pipe while an external reads it
/// (see the module's notes): polled by whoever waits for the external, dropped when it
/// is gone.
pub(crate) struct Feed(Rc<RefCell<Inner>>);

impl Feed {
    /// `Ready` once nothing is left to move.
    pub(crate) fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        flush(&self.0, cx);
        if self.0.borrow().backlog.is_empty() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        let mut p = self.0.borrow_mut();
        p.feeds -= 1;
        abandon(&mut p);
    }
}

impl PipeReader {
    /// See [`os_end`].
    pub(crate) fn os_end(&self, host: &dyn Host) -> Option<Rc<dyn HostFile>> {
        os_end(&self.0, true, host)
    }

    /// The feed an external given the OS read end needs: `None` without a backlog.
    pub(crate) fn feed(&self) -> Option<Feed> {
        let mut p = self.0.borrow_mut();
        if p.os.is_none() || p.backlog.is_empty() {
            return None;
        }
        p.feeds += 1;
        Some(Feed(self.0.clone()))
    }

    /// Whether the pipe is an OS pipe: bytes read from it cannot be given back in
    /// front of what an external reading the OS end takes next.
    pub(crate) fn is_os(&self) -> bool {
        self.0.borrow().os.is_some()
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
            // (no copy, no read-ahead to hold for a later pump), the backlog moved
            // into it first.
            let r = r.clone();
            drop(p);
            flush(&self.0, cx);
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

impl PipeReader {
    pub(crate) fn line_len(&self, delim: u8) -> Option<usize> {
        let p = self.0.borrow();
        let (front, back) = p.buf.as_slices();
        match front.iter().position(|&b| b == delim) {
            Some(i) => Some(i + 1),
            None => back
                .iter()
                .position(|&b| b == delim)
                .map(|i| front.len() + i + 1),
        }
        .or((!p.buf.is_empty()).then_some(p.buf.len()))
    }

    pub(crate) fn unread(&self, data: &[u8]) {
        let mut p = self.0.borrow_mut();
        for &b in data.iter().rev() {
            p.buf.push_front(b);
        }
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
            let behind = !p.backlog.is_empty();
            drop(p);
            if behind {
                flush(&self.0, cx);
                let mut p = self.0.borrow_mut();
                // Still a backlog: these bytes come behind it.
                if !p.backlog.is_empty() {
                    if data.is_empty() {
                        return Poll::Ready(Ok(0));
                    }
                    let space = PIPE_CAPACITY.saturating_sub(p.backlog.len());
                    if space == 0 {
                        register(&mut p.write_wakers, cx.waker());
                        return Poll::Pending;
                    }
                    let n = space.min(data.len());
                    p.backlog.extend(&data[..n]);
                    return Poll::Ready(Ok(n));
                }
            }
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
            abandon(&mut p);
            wake_all(&mut p.write_wakers);
        }
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        let mut p = self.0.borrow_mut();
        p.writers -= 1;
        if p.writers == 0 {
            release(&mut p);
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
