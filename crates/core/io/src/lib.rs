use std::{
    io::{self, IoSlice, SeekFrom},
    pin::Pin,
    task::{Context, Poll},
};

mod buf;
mod ext;
mod split;
mod util;

pub use buf::{BufReader, Lines, Take};
pub use ext::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
pub use split::{ReadHalf, WriteHalf, split};
pub use util::{Empty, Sink, copy, copy_bidirectional, empty, sink};

pub struct ReadBuf<'a> {
    buf: &'a mut [u8],
    filled: usize,
}

impl<'a> ReadBuf<'a> {
    pub const fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, filled: 0 }
    }

    pub const fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn filled(&self) -> &[u8] {
        &self.buf[..self.filled]
    }

    pub fn filled_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.filled]
    }

    pub const fn remaining(&self) -> usize {
        self.buf.len() - self.filled
    }

    pub fn initialize_unfilled(&mut self) -> &mut [u8] {
        &mut self.buf[self.filled..]
    }

    pub fn initialize_unfilled_to(&mut self, length: usize) -> &mut [u8] {
        assert!(
            self.remaining() >= length,
            "a read buffer has at least the requested room"
        );
        &mut self.buf[self.filled..self.filled + length]
    }

    pub fn advance(&mut self, length: usize) {
        assert!(
            self.remaining() >= length,
            "a read buffer advances within its capacity"
        );
        self.filled += length;
    }

    pub fn set_filled(&mut self, filled: usize) {
        assert!(
            filled <= self.buf.len(),
            "a read buffer is filled within its capacity"
        );
        self.filled = filled;
    }

    pub const fn clear(&mut self) {
        self.filled = 0;
    }

    pub fn put_slice(&mut self, bytes: &[u8]) {
        assert!(
            self.remaining() >= bytes.len(),
            "a read buffer has room for the slice put into it"
        );
        self.buf[self.filled..self.filled + bytes.len()].copy_from_slice(bytes);
        self.filled += bytes.len();
    }
}

pub trait AsyncRead {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>>;
}

pub trait AsyncWrite {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>>;

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>>;

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>>;

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let buf = bufs
            .iter()
            .find(|buf| !buf.is_empty())
            .map_or(&[][..], |buf| &**buf);
        self.poll_write(context, buf)
    }

    fn is_write_vectored(&self) -> bool {
        false
    }
}

pub trait AsyncBufRead: AsyncRead {
    fn poll_fill_buf(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<&[u8]>>;

    fn consume(self: Pin<&mut Self>, amount: usize);
}

pub trait AsyncSeek {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()>;

    fn poll_complete(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<u64>>;
}

#[cfg(not(target_family = "wasm"))]
mod objects {
    use super::{AsyncRead, AsyncWrite, IoSlice, Pin, ReadBuf};
    use std::{
        io,
        task::{Context, Poll},
    };

    type Reader = dyn AsyncRead + Send + Unpin;
    type Writer = dyn AsyncWrite + Send + Unpin;

    impl AsyncRead for Box<Reader> {
        fn poll_read(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_read(context, buf)
        }
    }

    impl AsyncWrite for Box<Writer> {
        fn poll_write(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut **self.get_mut()).poll_write(context, buf)
        }

        fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_flush(context)
        }

        fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_shutdown(context)
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut **self.get_mut()).poll_write_vectored(context, bufs)
        }

        fn is_write_vectored(&self) -> bool {
            (**self).is_write_vectored()
        }
    }
}

#[cfg(target_family = "wasm")]
mod pointers {
    use super::{AsyncBufRead, AsyncRead, AsyncWrite, IoSlice, Pin, ReadBuf};
    use std::{
        io,
        ops::DerefMut,
        task::{Context, Poll},
    };

    impl<P> AsyncRead for Pin<P>
    where
        P: DerefMut<Target: AsyncRead> + Unpin,
    {
        fn poll_read(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.get_mut().as_mut().poll_read(context, buf)
        }
    }

    impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for Box<T> {
        fn poll_read(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_read(context, buf)
        }
    }

    impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for &mut T {
        fn poll_read(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_read(context, buf)
        }
    }

    impl AsyncRead for &[u8] {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            let length = buf.remaining().min(this.len());
            let (head, tail) = this.split_at(length);
            buf.put_slice(head);
            *this = tail;
            Poll::Ready(Ok(()))
        }
    }

    impl<P> AsyncWrite for Pin<P>
    where
        P: DerefMut<Target: AsyncWrite> + Unpin,
    {
        fn poll_write(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.get_mut().as_mut().poll_write(context, buf)
        }

        fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.get_mut().as_mut().poll_flush(context)
        }

        fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.get_mut().as_mut().poll_shutdown(context)
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            self.get_mut().as_mut().poll_write_vectored(context, bufs)
        }

        fn is_write_vectored(&self) -> bool {
            (**self).is_write_vectored()
        }
    }

    impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for Box<T> {
        fn poll_write(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut **self.get_mut()).poll_write(context, buf)
        }

        fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_flush(context)
        }

        fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_shutdown(context)
        }
    }

    impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for &mut T {
        fn poll_write(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut **self.get_mut()).poll_write(context, buf)
        }

        fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_flush(context)
        }

        fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut **self.get_mut()).poll_shutdown(context)
        }
    }

    impl AsyncWrite for Vec<u8> {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.get_mut().extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl<T: AsyncBufRead + Unpin + ?Sized> AsyncBufRead for &mut T {
        fn poll_fill_buf(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<io::Result<&[u8]>> {
            Pin::new(&mut **self.get_mut()).poll_fill_buf(context)
        }

        fn consume(self: Pin<&mut Self>, amount: usize) {
            Pin::new(&mut **self.get_mut()).consume(amount);
        }
    }
}
