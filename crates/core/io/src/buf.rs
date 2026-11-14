use crate::{AsyncBufRead, AsyncRead, AsyncWrite, ReadBuf, ext};
use std::{
    io::{self, IoSlice},
    pin::Pin,
    task::{Context, Poll, ready},
};

const BUFFER_BYTES_DEFAULT: usize = 8 * 1024;

pub struct BufReader<R> {
    inner: R,
    buf: Box<[u8]>,
    position: usize,
    filled: usize,
}

impl<R> BufReader<R> {
    pub fn new(inner: R) -> Self {
        Self::with_capacity(BUFFER_BYTES_DEFAULT, inner)
    }

    pub fn with_capacity(capacity: usize, inner: R) -> Self {
        Self {
            inner,
            buf: vec![0; capacity].into_boxed_slice(),
            position: 0,
            filled: 0,
        }
    }

    pub const fn get_ref(&self) -> &R {
        &self.inner
    }

    pub const fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    pub const fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut R>
    where
        R: Unpin,
    {
        Pin::new(&mut self.get_mut().inner)
    }

    pub fn into_inner(self) -> R {
        self.inner
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buf[self.position..self.filled]
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for BufReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.position == this.filled && buf.remaining() >= this.buf.len() {
            let outcome = ready!(Pin::new(&mut this.inner).poll_read(context, buf));
            this.position = 0;
            this.filled = 0;
            return Poll::Ready(outcome);
        }
        let available = ready!(Pin::new(&mut *this).poll_fill_buf(context))?;
        let amount = available.len().min(buf.remaining());
        buf.put_slice(&available[..amount]);
        Pin::new(this).consume(amount);
        Poll::Ready(Ok(()))
    }
}

impl<R: AsyncRead + Unpin> AsyncBufRead for BufReader<R> {
    fn poll_fill_buf(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        if this.position >= this.filled {
            let mut buf = ReadBuf::new(&mut this.buf);
            ready!(Pin::new(&mut this.inner).poll_read(context, &mut buf))?;
            this.filled = buf.filled().len();
            this.position = 0;
        }
        Poll::Ready(Ok(&this.buf[this.position..this.filled]))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        let this = self.get_mut();
        this.position = (this.position + amount).min(this.filled);
    }
}

impl<R: AsyncWrite + Unpin> AsyncWrite for BufReader<R> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(context, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(context, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

pub struct Lines<R> {
    reader: R,
    line: String,
    bytes: Vec<u8>,
    read: usize,
}

impl<R> Lines<R> {
    pub(crate) const fn new(reader: R) -> Self {
        Self {
            reader,
            line: String::new(),
            bytes: Vec::new(),
            read: 0,
        }
    }

    pub const fn get_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    pub const fn get_ref(&self) -> &R {
        &self.reader
    }

    pub fn into_inner(self) -> R {
        self.reader
    }
}

impl<R: AsyncBufRead + Unpin> Lines<R> {
    pub fn poll_next_line(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<io::Result<Option<String>>> {
        let read = ready!(ext::poll_read_line(
            &mut self.reader,
            &mut self.line,
            &mut self.bytes,
            &mut self.read,
            context
        ))?;
        let mut line = std::mem::take(&mut self.line);
        if read == 0 && line.is_empty() {
            return Poll::Ready(Ok(None));
        }
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        Poll::Ready(Ok(Some(line)))
    }

    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        std::future::poll_fn(|context| self.poll_next_line(context)).await
    }
}

pub struct Take<'a, R: ?Sized> {
    inner: &'a mut R,
    limit: u64,
}

impl<'a, R: ?Sized> Take<'a, R> {
    pub(crate) const fn new(inner: &'a mut R, limit: u64) -> Self {
        Self { inner, limit }
    }

    pub const fn limit(&self) -> u64 {
        self.limit
    }

    pub const fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }
}

fn clamp(limit: u64, length: usize) -> usize {
    usize::try_from(limit).map_or(length, |limit| limit.min(length))
}

impl<R: AsyncRead + Unpin + ?Sized> AsyncRead for Take<'_, R> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.limit == 0 {
            return Poll::Ready(Ok(()));
        }
        let allowed = clamp(this.limit, buf.remaining());
        let mut limited = ReadBuf::new(buf.initialize_unfilled_to(allowed));
        ready!(Pin::new(&mut *this.inner).poll_read(context, &mut limited))?;
        let read = limited.filled().len();
        buf.advance(read);
        this.limit -= read as u64;
        Poll::Ready(Ok(()))
    }
}

impl<R: AsyncBufRead + Unpin + ?Sized> AsyncBufRead for Take<'_, R> {
    fn poll_fill_buf(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        if this.limit == 0 {
            return Poll::Ready(Ok(&[]));
        }
        let limit = this.limit;
        let available = ready!(Pin::new(&mut *this.inner).poll_fill_buf(context))?;
        let allowed = clamp(limit, available.len());
        Poll::Ready(Ok(&available[..allowed]))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        let this = self.get_mut();
        let amount = clamp(this.limit, amount);
        this.limit -= amount as u64;
        Pin::new(&mut *this.inner).consume(amount);
    }
}
