use std::{
    future::poll_fn,
    io::{self, IoSlice, Read, Write},
    net::{IpAddr, Shutdown, SocketAddr},
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
};

use nanocode_io::{AsyncRead, AsyncWrite, ReadBuf};
use nanocode_sys::AtomicWaker;

use crate::{
    reactor::{Direction, Ready, Registration, reactor},
    sys::AsRaw,
};

const LOOKUP_THREAD: &str = "nanocode-lookup";
#[cfg(windows)]
const CONNECT_THREAD: &str = "nanocode-connect";

pub struct Source<T: AsRaw> {
    inner: Option<T>,
    registration: Registration,
}

impl<T: AsRaw> Source<T> {
    pub fn new(inner: T) -> io::Result<Self> {
        let registration = reactor()?.register(inner.as_raw_source())?;
        Ok(Self {
            inner: Some(inner),
            registration,
        })
    }

    #[must_use]
    pub const fn get_ref(&self) -> &T {
        self.inner
            .as_ref()
            .expect("a source holds its descriptor until it is taken")
    }

    pub const fn get_mut(&mut self) -> &mut T {
        self.inner
            .as_mut()
            .expect("a source holds its descriptor until it is taken")
    }

    pub fn into_inner(mut self) -> io::Result<T> {
        let inner = self
            .inner
            .take()
            .expect("a source holds its descriptor until it is taken");
        self.registration.deregister(inner.as_raw_source())?;
        Ok(inner)
    }

    pub fn poll_io<R>(
        &self,
        context: &mut Context<'_>,
        direction: Direction,
        mut operation: impl FnMut(&T) -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        let inner = self.get_ref();
        self.registration
            .poll_io(context, direction, || operation(inner))
    }

    pub fn try_io<R>(
        &self,
        direction: Direction,
        operation: impl FnOnce(&T) -> io::Result<R>,
    ) -> io::Result<R> {
        let inner = self.get_ref();
        self.registration.try_io(direction, || operation(inner))
    }

    pub fn poll_ready(&self, context: &mut Context<'_>, direction: Direction) -> Poll<Ready> {
        self.registration.io().poll_ready(context, direction)
    }

    pub async fn ready(&self, direction: Direction) -> Ready {
        self.registration.ready(direction).await
    }

    pub fn clear(&self, ready: Ready) {
        self.registration.clear(ready);
    }
}

impl<T: AsRaw> Drop for Source<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.as_ref() {
            let _ = self.registration.deregister(inner.as_raw_source());
        }
    }
}

#[cfg(unix)]
impl<T: AsRaw> std::os::fd::AsFd for Source<T> {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.get_ref().as_raw_source()
    }
}

fn poll_read_with<T: AsRaw>(
    source: &Source<T>,
    context: &mut Context<'_>,
    buffer: &mut ReadBuf<'_>,
    read: impl Fn(&T, &mut [u8]) -> io::Result<usize>,
) -> Poll<io::Result<()>> {
    let unfilled = buffer.initialize_unfilled();
    let read = match source.poll_io(context, Direction::Read, |inner| read(inner, unfilled)) {
        Poll::Ready(Ok(read)) => read,
        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
        Poll::Pending => return Poll::Pending,
    };
    buffer.advance(read);
    Poll::Ready(Ok(()))
}

pub enum Resolve {
    Ready(Vec<SocketAddr>),
    Lookup(String),
    Failed(io::Error),
}

pub trait ToSocketAddrs {
    fn resolve(&self) -> Resolve;
}

impl ToSocketAddrs for SocketAddr {
    fn resolve(&self) -> Resolve {
        Resolve::Ready(vec![*self])
    }
}

impl ToSocketAddrs for [SocketAddr] {
    fn resolve(&self) -> Resolve {
        Resolve::Ready(self.to_vec())
    }
}

impl ToSocketAddrs for (IpAddr, u16) {
    fn resolve(&self) -> Resolve {
        Resolve::Ready(vec![SocketAddr::new(self.0, self.1)])
    }
}

impl ToSocketAddrs for (std::net::Ipv4Addr, u16) {
    fn resolve(&self) -> Resolve {
        Resolve::Ready(vec![SocketAddr::new(IpAddr::V4(self.0), self.1)])
    }
}

impl ToSocketAddrs for (std::net::Ipv6Addr, u16) {
    fn resolve(&self) -> Resolve {
        Resolve::Ready(vec![SocketAddr::new(IpAddr::V6(self.0), self.1)])
    }
}

impl ToSocketAddrs for (&str, u16) {
    fn resolve(&self) -> Resolve {
        match self.0.parse::<IpAddr>() {
            Ok(address) => Resolve::Ready(vec![SocketAddr::new(address, self.1)]),
            Err(_) => Resolve::Lookup(format!("{}:{}", self.0, self.1)),
        }
    }
}

impl ToSocketAddrs for (String, u16) {
    fn resolve(&self) -> Resolve {
        (self.0.as_str(), self.1).resolve()
    }
}

impl ToSocketAddrs for str {
    fn resolve(&self) -> Resolve {
        if let Ok(address) = self.parse::<SocketAddr>() {
            return Resolve::Ready(vec![address]);
        }
        let Some((_, port)) = self.rsplit_once(':') else {
            return Resolve::Failed(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid socket address",
            ));
        };
        if port.parse::<u16>().is_err() {
            return Resolve::Failed(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid port value",
            ));
        }
        Resolve::Lookup(self.to_owned())
    }
}

impl ToSocketAddrs for String {
    fn resolve(&self) -> Resolve {
        self.as_str().resolve()
    }
}

impl<T: ToSocketAddrs + ?Sized> ToSocketAddrs for &T {
    fn resolve(&self) -> Resolve {
        (**self).resolve()
    }
}

struct Answer<T> {
    value: OnceLock<io::Result<T>>,
    waker: AtomicWaker,
}

async fn on_thread<T: Send + Sync + 'static>(
    name: &str,
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let answer = Arc::new(Answer {
        value: OnceLock::new(),
        waker: AtomicWaker::new(),
    });
    let answering = answer.clone();
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _ = answering.value.set(work());
            answering.waker.wake();
        })?;
    poll_fn(|context| {
        if answer.value.get().is_some() {
            return Poll::Ready(());
        }
        answer.waker.register(context.waker());
        if answer.value.get().is_some() {
            return Poll::Ready(());
        }
        Poll::Pending
    })
    .await;
    Arc::into_inner(answer)
        .and_then(|answer| answer.value.into_inner())
        .unwrap_or_else(|| Err(io::Error::other("a finished thread holds its answer")))
}

async fn resolve(addresses: impl ToSocketAddrs) -> io::Result<Vec<SocketAddr>> {
    let host = match addresses.resolve() {
        Resolve::Ready(addresses) => return Ok(addresses),
        Resolve::Failed(error) => return Err(error),
        Resolve::Lookup(host) => host,
    };
    on_thread(LOOKUP_THREAD, move || {
        std::net::ToSocketAddrs::to_socket_addrs(&host).map(Iterator::collect)
    })
    .await
}

pub async fn lookup_host(host: impl ToSocketAddrs) -> io::Result<impl Iterator<Item = SocketAddr>> {
    resolve(host).await.map(Vec::into_iter)
}

fn no_addresses() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "could not resolve to any address",
    )
}

pub trait Shared: Send + Sync + 'static {
    fn poll_read_shared(
        &self,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>>;
    fn poll_write_shared(
        &self,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>>;
    fn poll_write_vectored_shared(
        &self,
        context: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>>;
    fn shutdown_write(&self) -> io::Result<()>;
}

pub struct ReadHalf<S: Shared> {
    stream: Arc<S>,
}

pub struct WriteHalf<S: Shared> {
    stream: Arc<S>,
    shutdown_on_drop: bool,
}

pub struct ReuniteError<S: Shared>(pub ReadHalf<S>, pub WriteHalf<S>);

impl<S: Shared> std::fmt::Debug for ReuniteError<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ReuniteError")
    }
}

impl<S: Shared> std::fmt::Display for ReuniteError<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("tried to reunite halves that are not from the same socket")
    }
}

impl<S: Shared> std::error::Error for ReuniteError<S> {}

impl<S: Shared> std::fmt::Debug for ReadHalf<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OwnedReadHalf")
    }
}

impl<S: Shared> std::fmt::Debug for WriteHalf<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OwnedWriteHalf")
    }
}

fn split<S: Shared>(stream: S) -> (ReadHalf<S>, WriteHalf<S>) {
    let shared = Arc::new(stream);
    (
        ReadHalf {
            stream: shared.clone(),
        },
        WriteHalf {
            stream: shared,
            shutdown_on_drop: true,
        },
    )
}

impl<S: Shared> ReadHalf<S> {
    pub fn reunite(self, mut other: WriteHalf<S>) -> Result<S, ReuniteError<S>> {
        if !Arc::ptr_eq(&self.stream, &other.stream) {
            return Err(ReuniteError(self, other));
        }
        other.shutdown_on_drop = false;
        let stream = other.stream.clone();
        drop(other);
        drop(self);
        Ok(Arc::into_inner(stream).expect("both halves of a stream were given back"))
    }
}

impl<S: Shared> AsRef<S> for ReadHalf<S> {
    fn as_ref(&self) -> &S {
        &self.stream
    }
}

impl<S: Shared> AsRef<S> for WriteHalf<S> {
    fn as_ref(&self) -> &S {
        &self.stream
    }
}

impl<S: Shared> WriteHalf<S> {
    pub fn forget(mut self) {
        self.shutdown_on_drop = false;
    }
}

impl<S: Shared> Drop for WriteHalf<S> {
    fn drop(&mut self) {
        if self.shutdown_on_drop {
            let _ = self.stream.shutdown_write();
        }
    }
}

impl<S: Shared> AsyncRead for ReadHalf<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.stream.poll_read_shared(context, buffer)
    }
}

impl<S: Shared> AsyncWrite for WriteHalf<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.stream.poll_write_shared(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = self.stream.shutdown_write();
        if result.is_ok() {
            self.shutdown_on_drop = false;
        }
        Poll::Ready(result)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.stream.poll_write_vectored_shared(context, buffers)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }
}

macro_rules! stream_io {
    ($stream:ty) => {
        impl Shared for $stream {
            fn poll_read_shared(
                &self,
                context: &mut Context<'_>,
                buffer: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                poll_read_with(&self.source, context, buffer, |mut inner, bytes| {
                    inner.read(bytes)
                })
            }

            fn poll_write_shared(
                &self,
                context: &mut Context<'_>,
                buffer: &[u8],
            ) -> Poll<io::Result<usize>> {
                self.source
                    .poll_io(context, Direction::Write, |mut inner| inner.write(buffer))
            }

            fn poll_write_vectored_shared(
                &self,
                context: &mut Context<'_>,
                buffers: &[IoSlice<'_>],
            ) -> Poll<io::Result<usize>> {
                self.source.poll_io(context, Direction::Write, |mut inner| {
                    inner.write_vectored(buffers)
                })
            }

            fn shutdown_write(&self) -> io::Result<()> {
                self.source.get_ref().shutdown(Shutdown::Write)
            }
        }

        impl AsyncRead for $stream {
            fn poll_read(
                self: Pin<&mut Self>,
                context: &mut Context<'_>,
                buffer: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                self.poll_read_shared(context, buffer)
            }
        }

        impl AsyncWrite for $stream {
            fn poll_write(
                self: Pin<&mut Self>,
                context: &mut Context<'_>,
                buffer: &[u8],
            ) -> Poll<io::Result<usize>> {
                self.poll_write_shared(context, buffer)
            }

            fn poll_flush(
                self: Pin<&mut Self>,
                _context: &mut Context<'_>,
            ) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }

            fn poll_shutdown(
                self: Pin<&mut Self>,
                _context: &mut Context<'_>,
            ) -> Poll<io::Result<()>> {
                Poll::Ready(self.shutdown_write())
            }

            fn poll_write_vectored(
                self: Pin<&mut Self>,
                context: &mut Context<'_>,
                buffers: &[IoSlice<'_>],
            ) -> Poll<io::Result<usize>> {
                self.poll_write_vectored_shared(context, buffers)
            }

            fn is_write_vectored(&self) -> bool {
                true
            }
        }

        impl $stream {
            pub async fn readable(&self) -> io::Result<()> {
                self.source.ready(Direction::Read).await;
                Ok(())
            }

            pub async fn writable(&self) -> io::Result<()> {
                self.source.ready(Direction::Write).await;
                Ok(())
            }

            pub fn try_read(&self, buffer: &mut [u8]) -> io::Result<usize> {
                self.source
                    .try_io(Direction::Read, |mut inner| inner.read(buffer))
            }

            pub fn try_write(&self, buffer: &[u8]) -> io::Result<usize> {
                self.source
                    .try_io(Direction::Write, |mut inner| inner.write(buffer))
            }

            pub fn into_split(self) -> (ReadHalf<Self>, WriteHalf<Self>) {
                split(self)
            }
        }
    };
}

pub mod tcp {
    pub type OwnedReadHalf = super::ReadHalf<super::TcpStream>;
    pub type OwnedWriteHalf = super::WriteHalf<super::TcpStream>;
    pub type ReuniteError = super::ReuniteError<super::TcpStream>;
}

pub struct TcpStream {
    source: Source<std::net::TcpStream>,
}

stream_io!(TcpStream);

impl TcpStream {
    pub async fn connect(addresses: impl ToSocketAddrs) -> io::Result<Self> {
        let addresses = resolve(addresses).await?;
        let mut last = None;
        for address in addresses {
            match Self::connect_one(address).await {
                Ok(stream) => return Ok(stream),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(no_addresses))
    }

    #[cfg(unix)]
    async fn connect_one(address: SocketAddr) -> io::Result<Self> {
        use nanocode_sys::unix::poll as sys;
        let (socket, waiting) = match sys::connect_nonblocking(&address)? {
            sys::Connecting::Connected(socket) => (socket, false),
            sys::Connecting::InProgress(socket) => (socket, true),
        };
        let stream = Self {
            source: Source::new(std::net::TcpStream::from(socket))?,
        };
        if waiting {
            stream.source.ready(Direction::Write).await;
            if let Some(error) = sys::socket_error(stream.source.get_ref())? {
                return Err(error);
            }
            if let Err(error) = stream.source.get_ref().peer_addr()
                && error.kind() == io::ErrorKind::NotConnected
            {
                return Err(error);
            }
        }
        Ok(stream)
    }

    #[cfg(windows)]
    async fn connect_one(address: SocketAddr) -> io::Result<Self> {
        let stream = on_thread(CONNECT_THREAD, move || {
            std::net::TcpStream::connect(address)
        })
        .await?;
        Self::from_std(stream)
    }

    pub fn from_std(stream: std::net::TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            source: Source::new(stream)?,
        })
    }

    pub fn into_std(self) -> io::Result<std::net::TcpStream> {
        let stream = self.source.into_inner()?;
        stream.set_nonblocking(false)?;
        Ok(stream)
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.source.get_ref().set_nodelay(nodelay)
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        self.source.get_ref().nodelay()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.source.get_ref().local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.source.get_ref().peer_addr()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.source.get_ref().take_error()
    }

    pub fn poll_peek(
        &self,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<usize>> {
        let unfilled = buffer.initialize_unfilled();
        match self
            .source
            .poll_io(context, Direction::Read, |inner| inner.peek(unfilled))
        {
            Poll::Ready(Ok(read)) => {
                buffer.advance(read);
                Poll::Ready(Ok(read))
            }
            other => other,
        }
    }

    pub async fn peek(&self, buffer: &mut [u8]) -> io::Result<usize> {
        poll_fn(|context| {
            self.source
                .poll_io(context, Direction::Read, |inner| inner.peek(buffer))
        })
        .await
    }
}

#[cfg(unix)]
impl std::os::fd::AsFd for TcpStream {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self.source)
    }
}

#[cfg(unix)]
impl std::os::fd::AsRawFd for TcpStream {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        std::os::fd::AsRawFd::as_raw_fd(&std::os::fd::AsFd::as_fd(self))
    }
}

impl TryFrom<std::net::TcpStream> for TcpStream {
    type Error = io::Error;

    fn try_from(stream: std::net::TcpStream) -> io::Result<Self> {
        Self::from_std(stream)
    }
}

impl std::fmt::Debug for TcpStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.get_ref().fmt(formatter)
    }
}

pub struct TcpListener {
    source: Source<std::net::TcpListener>,
}

impl TcpListener {
    pub async fn bind(addresses: impl ToSocketAddrs) -> io::Result<Self> {
        let addresses = resolve(addresses).await?;
        let mut last = None;
        for address in addresses {
            match std::net::TcpListener::bind(address).and_then(Self::from_std) {
                Ok(listener) => return Ok(listener),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(no_addresses))
    }

    pub fn from_std(listener: std::net::TcpListener) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        Ok(Self {
            source: Source::new(listener)?,
        })
    }

    pub fn into_std(self) -> io::Result<std::net::TcpListener> {
        let listener = self.source.into_inner()?;
        listener.set_nonblocking(false)?;
        Ok(listener)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.source.get_ref().local_addr()
    }

    pub fn poll_accept(
        &self,
        context: &mut Context<'_>,
    ) -> Poll<io::Result<(TcpStream, SocketAddr)>> {
        match self
            .source
            .poll_io(context, Direction::Read, std::net::TcpListener::accept)
        {
            Poll::Ready(Ok((stream, address))) => {
                Poll::Ready(TcpStream::from_std(stream).map(|stream| (stream, address)))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        poll_fn(|context| self.poll_accept(context)).await
    }
}

impl TryFrom<std::net::TcpListener> for TcpListener {
    type Error = io::Error;

    fn try_from(listener: std::net::TcpListener) -> io::Result<Self> {
        Self::from_std(listener)
    }
}

impl std::fmt::Debug for TcpListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.get_ref().fmt(formatter)
    }
}

#[cfg(unix)]
impl std::os::fd::AsFd for TcpListener {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self.source)
    }
}

#[cfg(unix)]
pub use unix::{UnixDatagram, UnixListener, UnixStream};

#[cfg(unix)]
pub mod unix {
    use std::{
        future::poll_fn,
        io::{self, IoSlice, Read, Write},
        net::Shutdown,
        os::{
            fd::{AsFd, BorrowedFd, OwnedFd},
            unix::net as unix_net,
        },
        path::Path,
        pin::Pin,
        task::{Context, Poll},
    };

    use nanocode_io::{AsyncRead, AsyncWrite, ReadBuf};

    use super::{ReadHalf, Shared, Source, WriteHalf, poll_read_with, split};
    use crate::reactor::Direction;

    pub use std::os::unix::net::SocketAddr;

    pub type OwnedReadHalf = ReadHalf<UnixStream>;
    pub type OwnedWriteHalf = WriteHalf<UnixStream>;

    pub struct UnixStream {
        source: Source<unix_net::UnixStream>,
    }

    stream_io!(UnixStream);

    impl UnixStream {
        pub async fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
            Self::from_std(unix_net::UnixStream::connect(path)?)
        }

        pub fn pair() -> io::Result<(Self, Self)> {
            let (left, right) = unix_net::UnixStream::pair()?;
            Ok((Self::from_std(left)?, Self::from_std(right)?))
        }

        pub fn from_std(stream: unix_net::UnixStream) -> io::Result<Self> {
            stream.set_nonblocking(true)?;
            Ok(Self {
                source: Source::new(stream)?,
            })
        }

        pub fn into_std(self) -> io::Result<unix_net::UnixStream> {
            let stream = self.source.into_inner()?;
            stream.set_nonblocking(false)?;
            Ok(stream)
        }

        pub fn local_addr(&self) -> io::Result<SocketAddr> {
            self.source.get_ref().local_addr()
        }

        pub fn peer_addr(&self) -> io::Result<SocketAddr> {
            self.source.get_ref().peer_addr()
        }
    }

    impl AsFd for UnixStream {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.source.as_fd()
        }
    }

    impl std::fmt::Debug for UnixStream {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.source.get_ref().fmt(formatter)
        }
    }

    pub struct UnixListener {
        source: Source<unix_net::UnixListener>,
    }

    impl UnixListener {
        pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
            Self::from_std(unix_net::UnixListener::bind(path)?)
        }

        pub fn from_std(listener: unix_net::UnixListener) -> io::Result<Self> {
            listener.set_nonblocking(true)?;
            Ok(Self {
                source: Source::new(listener)?,
            })
        }

        pub fn into_std(self) -> io::Result<unix_net::UnixListener> {
            let listener = self.source.into_inner()?;
            listener.set_nonblocking(false)?;
            Ok(listener)
        }

        pub fn local_addr(&self) -> io::Result<SocketAddr> {
            self.source.get_ref().local_addr()
        }

        pub fn poll_accept(
            &self,
            context: &mut Context<'_>,
        ) -> Poll<io::Result<(UnixStream, SocketAddr)>> {
            match self
                .source
                .poll_io(context, Direction::Read, unix_net::UnixListener::accept)
            {
                Poll::Ready(Ok((stream, address))) => {
                    Poll::Ready(UnixStream::from_std(stream).map(|stream| (stream, address)))
                }
                Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                Poll::Pending => Poll::Pending,
            }
        }

        pub async fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
            poll_fn(|context| self.poll_accept(context)).await
        }
    }

    impl AsFd for UnixListener {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.source.as_fd()
        }
    }

    impl std::fmt::Debug for UnixListener {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.source.get_ref().fmt(formatter)
        }
    }

    pub struct UnixDatagram {
        source: Source<unix_net::UnixDatagram>,
    }

    impl UnixDatagram {
        pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
            Self::from_std(unix_net::UnixDatagram::bind(path)?)
        }

        pub fn unbound() -> io::Result<Self> {
            Self::from_std(unix_net::UnixDatagram::unbound()?)
        }

        pub fn pair() -> io::Result<(Self, Self)> {
            let (left, right) = unix_net::UnixDatagram::pair()?;
            Ok((Self::from_std(left)?, Self::from_std(right)?))
        }

        pub fn from_std(socket: unix_net::UnixDatagram) -> io::Result<Self> {
            socket.set_nonblocking(true)?;
            Ok(Self {
                source: Source::new(socket)?,
            })
        }

        pub fn connect(&self, path: impl AsRef<Path>) -> io::Result<()> {
            self.source.get_ref().connect(path)
        }

        pub fn local_addr(&self) -> io::Result<SocketAddr> {
            self.source.get_ref().local_addr()
        }

        pub async fn send(&self, buffer: &[u8]) -> io::Result<usize> {
            poll_fn(|context| {
                self.source
                    .poll_io(context, Direction::Write, |inner| inner.send(buffer))
            })
            .await
        }

        pub async fn recv(&self, buffer: &mut [u8]) -> io::Result<usize> {
            poll_fn(|context| {
                self.source
                    .poll_io(context, Direction::Read, |inner| inner.recv(buffer))
            })
            .await
        }

        pub async fn send_to(&self, buffer: &[u8], path: impl AsRef<Path>) -> io::Result<usize> {
            let path = path.as_ref();
            poll_fn(|context| {
                self.source.poll_io(context, Direction::Write, |inner| {
                    inner.send_to(buffer, path)
                })
            })
            .await
        }

        pub async fn recv_from(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
            poll_fn(|context| {
                self.source
                    .poll_io(context, Direction::Read, |inner| inner.recv_from(buffer))
            })
            .await
        }

        pub fn try_send(&self, buffer: &[u8]) -> io::Result<usize> {
            self.source
                .try_io(Direction::Write, |inner| inner.send(buffer))
        }

        pub fn try_recv(&self, buffer: &mut [u8]) -> io::Result<usize> {
            self.source
                .try_io(Direction::Read, |inner| inner.recv(buffer))
        }
    }

    impl AsFd for UnixDatagram {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.source.as_fd()
        }
    }

    pub struct Pipe {
        source: Source<std::fs::File>,
    }

    impl Pipe {
        pub fn from_owned_fd(fd: OwnedFd) -> io::Result<Self> {
            nanocode_sys::unix::set_nonblocking(&fd, true)?;
            Ok(Self {
                source: Source::new(std::fs::File::from(fd))?,
            })
        }

        pub fn into_owned_fd(self) -> io::Result<OwnedFd> {
            let file = self.source.into_inner()?;
            let fd = OwnedFd::from(file);
            nanocode_sys::unix::set_nonblocking(&fd, false)?;
            Ok(fd)
        }
    }

    impl AsFd for Pipe {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.source.as_fd()
        }
    }

    impl AsyncRead for Pipe {
        fn poll_read(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            poll_read_with(&self.source, context, buffer, |mut inner, bytes| {
                inner.read(bytes)
            })
        }
    }

    impl AsyncWrite for Pipe {
        fn poll_write(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.source
                .poll_io(context, Direction::Write, |mut inner| inner.write(buffer))
        }

        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffers: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            self.source.poll_io(context, Direction::Write, |mut inner| {
                inner.write_vectored(buffers)
            })
        }

        fn is_write_vectored(&self) -> bool {
            true
        }
    }

    impl std::fmt::Debug for Pipe {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.source.get_ref().fmt(formatter)
        }
    }
}
