use crate::{AsyncRead, AsyncWrite, ReadBuf};
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};

const COPY_BUFFER_BYTES: usize = 8 * 1024;

struct Transfer {
    buf: Box<[u8]>,
    position: usize,
    filled: usize,
    copied: u64,
    read_done: bool,
    need_flush: bool,
}

impl Transfer {
    fn new() -> Self {
        Self {
            buf: vec![0; COPY_BUFFER_BYTES].into_boxed_slice(),
            position: 0,
            filled: 0,
            copied: 0,
            read_done: false,
            need_flush: false,
        }
    }

    fn poll_fill<R: AsyncRead + ?Sized>(
        &mut self,
        context: &mut Context<'_>,
        reader: Pin<&mut R>,
    ) -> Poll<io::Result<()>> {
        let mut buf = ReadBuf::new(&mut self.buf);
        buf.set_filled(self.filled);
        let polled = reader.poll_read(context, &mut buf);
        if let Poll::Ready(Ok(())) = polled {
            let filled = buf.filled().len();
            self.read_done = self.filled == filled;
            self.filled = filled;
        }
        polled
    }

    fn poll_write_some<R, W>(
        &mut self,
        context: &mut Context<'_>,
        reader: Pin<&mut R>,
        writer: Pin<&mut W>,
    ) -> Poll<io::Result<usize>>
    where
        R: AsyncRead + ?Sized,
        W: AsyncWrite + ?Sized,
    {
        match writer.poll_write(context, &self.buf[self.position..self.filled]) {
            Poll::Pending => {
                if !self.read_done && self.filled < self.buf.len() {
                    ready!(self.poll_fill(context, reader))?;
                }
                Poll::Pending
            }
            polled => polled,
        }
    }

    fn poll_copy<R, W>(
        &mut self,
        context: &mut Context<'_>,
        mut reader: Pin<&mut R>,
        mut writer: Pin<&mut W>,
    ) -> Poll<io::Result<u64>>
    where
        R: AsyncRead + ?Sized,
        W: AsyncWrite + ?Sized,
    {
        loop {
            if self.filled < self.buf.len() && !self.read_done {
                match self.poll_fill(context, reader.as_mut()) {
                    Poll::Ready(Ok(())) => {}
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Pending => {
                        if self.position == self.filled {
                            if self.need_flush {
                                ready!(writer.as_mut().poll_flush(context))?;
                                self.need_flush = false;
                            }
                            return Poll::Pending;
                        }
                    }
                }
            }
            while self.position < self.filled {
                let written =
                    ready!(self.poll_write_some(context, reader.as_mut(), writer.as_mut()))?;
                if written == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "write zero byte into writer",
                    )));
                }
                self.position += written;
                self.copied += written as u64;
                self.need_flush = true;
            }
            assert!(
                self.position <= self.filled,
                "a writer writes at most the slice it is given"
            );
            self.position = 0;
            self.filled = 0;
            if self.read_done {
                ready!(writer.as_mut().poll_flush(context))?;
                return Poll::Ready(Ok(self.copied));
            }
        }
    }
}

pub struct Copy<'a, R: ?Sized, W: ?Sized> {
    reader: &'a mut R,
    writer: &'a mut W,
    transfer: Transfer,
}

impl<R, W> Future for Copy<'_, R, W>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    type Output = io::Result<u64>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.transfer.poll_copy(
            context,
            Pin::new(&mut *this.reader),
            Pin::new(&mut *this.writer),
        )
    }
}

pub fn copy<'a, R, W>(reader: &'a mut R, writer: &'a mut W) -> Copy<'a, R, W>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    Copy {
        reader,
        writer,
        transfer: Transfer::new(),
    }
}

enum Direction {
    Running(Transfer),
    ShuttingDown(u64),
    Done(u64),
}

fn poll_direction<R, W>(
    direction: &mut Direction,
    context: &mut Context<'_>,
    mut reader: Pin<&mut R>,
    mut writer: Pin<&mut W>,
) -> Poll<io::Result<u64>>
where
    R: AsyncRead + ?Sized,
    W: AsyncWrite + ?Sized,
{
    loop {
        match direction {
            Direction::Running(transfer) => {
                let copied = ready!(transfer.poll_copy(context, reader.as_mut(), writer.as_mut()))?;
                *direction = Direction::ShuttingDown(copied);
            }
            Direction::ShuttingDown(copied) => {
                let copied = *copied;
                ready!(writer.as_mut().poll_shutdown(context))?;
                *direction = Direction::Done(copied);
            }
            Direction::Done(copied) => return Poll::Ready(Ok(*copied)),
        }
    }
}

pub struct CopyBidirectional<'a, A: ?Sized, B: ?Sized> {
    a: &'a mut A,
    b: &'a mut B,
    a_to_b: Direction,
    b_to_a: Direction,
}

impl<A, B> Future for CopyBidirectional<'_, A, B>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    type Output = io::Result<(u64, u64)>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let a_to_b = poll_direction(
            &mut this.a_to_b,
            context,
            Pin::new(&mut *this.a),
            Pin::new(&mut *this.b),
        )?;
        let b_to_a = poll_direction(
            &mut this.b_to_a,
            context,
            Pin::new(&mut *this.b),
            Pin::new(&mut *this.a),
        )?;
        let (Poll::Ready(a_to_b), Poll::Ready(b_to_a)) = (a_to_b, b_to_a) else {
            return Poll::Pending;
        };
        Poll::Ready(Ok((a_to_b, b_to_a)))
    }
}

pub fn copy_bidirectional<'a, A, B>(a: &'a mut A, b: &'a mut B) -> CopyBidirectional<'a, A, B>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    CopyBidirectional {
        a,
        b,
        a_to_b: Direction::Running(Transfer::new()),
        b_to_a: Direction::Running(Transfer::new()),
    }
}

#[derive(Debug, Default)]
pub struct Sink;

#[must_use]
pub const fn sink() -> Sink {
    Sink
}

#[derive(Debug, Default)]
pub struct Empty;

#[must_use]
pub const fn empty() -> Empty {
    Empty
}

impl AsyncWrite for Sink {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Empty {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
