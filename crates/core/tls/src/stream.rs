use nanocode_io::{AsyncRead, AsyncWrite, ReadBuf};
use rustls::{
    ClientConfig, ClientConnection, ConnectionCommon, ServerConfig, ServerConnection, SideData,
    pki_types::ServerName,
};
use std::{
    future::poll_fn,
    io::{self, BufRead, IoSlice, Read, Write},
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
};

const HANDSHAKE_EOF: &str = "tls handshake eof";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shutdown {
    Open,
    Read,
    Write,
    Both,
}

impl Shutdown {
    const fn read(self) -> Self {
        match self {
            Self::Write | Self::Both => Self::Both,
            Self::Open | Self::Read => Self::Read,
        }
    }

    const fn write(self) -> Self {
        match self {
            Self::Read | Self::Both => Self::Both,
            Self::Open | Self::Write => Self::Write,
        }
    }

    const fn readable(self) -> bool {
        matches!(self, Self::Open | Self::Write)
    }

    const fn writeable(self) -> bool {
        matches!(self, Self::Open | Self::Read)
    }
}

pub struct TlsStream<S, C> {
    io: S,
    session: C,
    shutdown: Shutdown,
    need_flush: bool,
}

pub type ClientStream<S> = TlsStream<S, ClientConnection>;
pub type ServerStream<S> = TlsStream<S, ServerConnection>;

pub async fn connect<S>(
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
    io: S,
) -> io::Result<ClientStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let session = ClientConnection::new(config, name).map_err(io::Error::other)?;
    TlsStream::new(io, session).handshaken().await
}

pub async fn accept<S>(config: Arc<ServerConfig>, io: S) -> io::Result<ServerStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let session = ServerConnection::new(config).map_err(io::Error::other)?;
    TlsStream::new(io, session).handshaken().await
}

impl<S, C> TlsStream<S, C> {
    pub const fn get_ref(&self) -> (&S, &C) {
        (&self.io, &self.session)
    }

    pub const fn get_mut(&mut self) -> (&mut S, &mut C) {
        (&mut self.io, &mut self.session)
    }

    pub fn into_inner(self) -> (S, C) {
        (self.io, self.session)
    }
}

impl<S, C, D> TlsStream<S, C>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: DerefMut + Deref<Target = ConnectionCommon<D>> + Unpin,
    D: SideData,
{
    const fn new(io: S, session: C) -> Self {
        Self {
            io,
            session,
            shutdown: Shutdown::Open,
            need_flush: false,
        }
    }

    async fn handshaken(mut self) -> io::Result<Self> {
        poll_fn(|cx| self.poll_handshaken(cx)).await?;
        Ok(self)
    }

    fn poll_handshaken(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut pass = Pass::new(&mut self.io, &mut self.session, !self.shutdown.readable());
        pass.need_flush = self.need_flush;
        while pass.session.is_handshaking() {
            match pass.handshake(cx) {
                Poll::Ready(Ok(_)) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {
                    self.need_flush = pass.need_flush;
                    return Poll::Pending;
                }
            }
        }
        match pass.poll_flush(cx) {
            Poll::Pending => {
                self.need_flush = pass.need_flush;
                Poll::Pending
            }
            ready => ready,
        }
    }

    const fn pass(&mut self) -> Pass<'_, S, C> {
        let mut pass = Pass::new(&mut self.io, &mut self.session, !self.shutdown.readable());
        pass.need_flush = self.need_flush;
        pass
    }
}

impl<S, C, D> AsyncRead for TlsStream<S, C>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: DerefMut + Deref<Target = ConnectionCommon<D>> + Unpin,
    D: SideData,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.shutdown.readable() {
            return Poll::Ready(Ok(()));
        }
        let pass = Pass::new(&mut this.io, &mut this.session, false);
        let filled = match pass.poll_fill_buf(cx) {
            Poll::Ready(Ok([])) => None,
            Poll::Ready(Ok(data)) => {
                let amount = buf.remaining().min(data.len());
                buf.put_slice(&data[..amount]);
                Some(Ok(amount))
            }
            Poll::Ready(Err(error)) => Some(Err(error)),
            Poll::Pending => return Poll::Pending,
        };
        match filled {
            None => {
                this.shutdown = this.shutdown.read();
                Poll::Ready(Ok(()))
            }
            Some(Ok(amount)) => {
                this.session.reader().consume(amount);
                Poll::Ready(Ok(()))
            }
            Some(Err(error)) => {
                if error.kind() == io::ErrorKind::ConnectionAborted {
                    this.shutdown = this.shutdown.read();
                }
                Poll::Ready(Err(error))
            }
        }
    }
}

impl<S, C, D> AsyncWrite for TlsStream<S, C>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: DerefMut + Deref<Target = ConnectionCommon<D>> + Unpin,
    D: SideData,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut().pass().poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.get_mut().pass().poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().pass().poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.shutdown.writeable() {
            this.session.send_close_notify();
            this.shutdown = this.shutdown.write();
        }
        Pass::new(&mut this.io, &mut this.session, !this.shutdown.readable()).poll_shutdown(cx)
    }
}

struct Pass<'a, S, C> {
    io: &'a mut S,
    session: &'a mut C,
    eof: bool,
    need_flush: bool,
}

impl<'a, S, C, D> Pass<'a, S, C>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: DerefMut + Deref<Target = ConnectionCommon<D>>,
    D: SideData,
{
    const fn new(io: &'a mut S, session: &'a mut C, eof: bool) -> Self {
        Self {
            io,
            session,
            eof,
            need_flush: false,
        }
    }

    fn read_io(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        let read = match self.session.read_tls(&mut SyncRead { io: self.io, cx }) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Poll::Pending,
            Err(error) => return Poll::Ready(Err(error)),
        };
        if let Err(error) = self.session.process_new_packets() {
            drop(self.write_io(cx));
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, error)));
        }
        Poll::Ready(Ok(read))
    }

    fn write_io(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        match self.session.write_tls(&mut SyncWrite { io: self.io, cx }) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
            result => Poll::Ready(result),
        }
    }

    fn handshake(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<(usize, usize)>> {
        let mut written = 0;
        let mut read = 0;
        loop {
            let mut write_blocked = false;
            let mut read_blocked = false;
            while self.session.wants_write() {
                match self.write_io(cx) {
                    Poll::Ready(Ok(0)) => return Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
                    Poll::Ready(Ok(count)) => {
                        written += count;
                        self.need_flush = true;
                    }
                    Poll::Pending => {
                        write_blocked = true;
                        break;
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                }
            }
            if self.need_flush {
                match Pin::new(&mut *self.io).poll_flush(cx) {
                    Poll::Ready(Ok(())) => self.need_flush = false,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Pending => write_blocked = true,
                }
            }
            while !self.eof && self.session.wants_read() {
                match self.read_io(cx) {
                    Poll::Ready(Ok(0)) => self.eof = true,
                    Poll::Ready(Ok(count)) => read += count,
                    Poll::Pending => {
                        read_blocked = true;
                        break;
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                }
            }
            return match (self.eof, self.session.is_handshaking()) {
                (true, true) => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    HANDSHAKE_EOF,
                ))),
                (_, false) => Poll::Ready(Ok((read, written))),
                (_, true) if write_blocked || read_blocked => {
                    if read != 0 || written != 0 {
                        Poll::Ready(Ok((read, written)))
                    } else {
                        Poll::Pending
                    }
                }
                (_, true) => continue,
            };
        }
    }

    fn poll_fill_buf(mut self, cx: &mut Context<'_>) -> Poll<io::Result<&'a [u8]>>
    where
        D: 'a,
    {
        let mut io_pending = false;
        while !self.eof && self.session.wants_read() {
            match self.read_io(cx) {
                Poll::Ready(Ok(0)) => break,
                Poll::Ready(Ok(_)) => {}
                Poll::Pending => {
                    io_pending = true;
                    break;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            }
        }
        match self.session.reader().into_first_chunk() {
            Ok(data) => Poll::Ready(Ok(data)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if !io_pending {
                    cx.waker().wake_by_ref();
                }
                Poll::Pending
            }
            Err(error) => Poll::Ready(Err(error)),
        }
    }

    fn drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<bool>> {
        while self.session.wants_write() {
            match self.write_io(cx) {
                Poll::Ready(Ok(0)) | Poll::Pending => return Poll::Ready(Ok(true)),
                Poll::Ready(Ok(_)) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            }
        }
        Poll::Ready(Ok(false))
    }

    fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut position = 0;
        while position != buf.len() {
            position += self.session.writer().write(&buf[position..])?;
            let blocked = ready!(self.drain(cx))?;
            match (position, blocked) {
                (0, true) => return Poll::Pending,
                (written, true) => return Poll::Ready(Ok(written)),
                (_, false) => {}
            }
        }
        Poll::Ready(Ok(position))
    }

    fn poll_write_vectored(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if bufs.iter().all(|buf| buf.is_empty()) {
            return Poll::Ready(Ok(0));
        }
        loop {
            let written = self.session.writer().write_vectored(bufs)?;
            let blocked = ready!(self.drain(cx))?;
            match (written, blocked) {
                (0, true) => return Poll::Pending,
                (0, false) => {}
                (written, _) => return Poll::Ready(Ok(written)),
            }
        }
    }

    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.session.writer().flush()?;
        while self.session.wants_write() {
            if ready!(self.write_io(cx))? == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
        }
        Pin::new(&mut *self.io).poll_flush(cx)
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.session.wants_write() {
            if ready!(self.write_io(cx))? == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
        }
        Poll::Ready(match ready!(Pin::new(&mut *self.io).poll_shutdown(cx)) {
            Err(error) if error.kind() == io::ErrorKind::NotConnected => Ok(()),
            result => result,
        })
    }
}

struct SyncRead<'a, 'b, S> {
    io: &'a mut S,
    cx: &'a mut Context<'b>,
}

impl<S: AsyncRead + Unpin> Read for SyncRead<'_, '_, S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut buf = ReadBuf::new(buf);
        match Pin::new(&mut *self.io).poll_read(self.cx, &mut buf) {
            Poll::Ready(Ok(())) => Ok(buf.filled().len()),
            Poll::Ready(Err(error)) => Err(error),
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }
}

struct SyncWrite<'a, 'b, S> {
    io: &'a mut S,
    cx: &'a mut Context<'b>,
}

impl<S: AsyncWrite + Unpin> SyncWrite<'_, '_, S> {
    fn ready<T>(poll: Poll<io::Result<T>>) -> io::Result<T> {
        match poll {
            Poll::Ready(result) => result,
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }
}

impl<S: AsyncWrite + Unpin> Write for SyncWrite<'_, '_, S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Self::ready(Pin::new(&mut *self.io).poll_write(self.cx, buf))
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        Self::ready(Pin::new(&mut *self.io).poll_write_vectored(self.cx, bufs))
    }

    fn flush(&mut self) -> io::Result<()> {
        Self::ready(Pin::new(&mut *self.io).poll_flush(self.cx))
    }
}
