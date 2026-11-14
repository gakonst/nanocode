use crate::{AsyncRead, AsyncWrite, ReadBuf};
use nanocode_sys::BiLock;
use std::{
    io::{self, IoSlice},
    pin::Pin,
    task::{Context, Poll, ready},
};

pub struct ReadHalf<T> {
    lock: BiLock<T>,
}

pub struct WriteHalf<T> {
    lock: BiLock<T>,
}

pub fn split<T: AsyncRead + AsyncWrite>(stream: T) -> (ReadHalf<T>, WriteHalf<T>) {
    let (read, write) = BiLock::new(stream);
    (ReadHalf { lock: read }, WriteHalf { lock: write })
}

impl<T> ReadHalf<T> {
    pub fn is_pair_of(&self, other: &WriteHalf<T>) -> bool {
        self.lock.is_pair_of(&other.lock)
    }
}

impl<T: AsyncRead> AsyncRead for ReadHalf<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut guard = ready!(self.lock.poll_lock(context));
        guard.as_pin_mut().poll_read(context, buf)
    }
}

impl<T: AsyncWrite> AsyncWrite for WriteHalf<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut guard = ready!(self.lock.poll_lock(context));
        guard.as_pin_mut().poll_write(context, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut guard = ready!(self.lock.poll_lock(context));
        guard.as_pin_mut().poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut guard = ready!(self.lock.poll_lock(context));
        guard.as_pin_mut().poll_shutdown(context)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let mut guard = ready!(self.lock.poll_lock(context));
        guard.as_pin_mut().poll_write_vectored(context, bufs)
    }
}
