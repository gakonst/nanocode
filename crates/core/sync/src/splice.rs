use crate::future::{Either, join, select};
use nanocode_executor::TcpStream;
use nanocode_io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, split};
use nanocode_platform::{Instant, sleep_until};
use std::{
    io::IoSlice,
    pin::{Pin, pin},
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll},
    time::Duration,
};

pub trait Duplex: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> Duplex for T {}

impl AsyncRead for Box<dyn Duplex> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_read(context, buf)
    }
}

impl AsyncWrite for Box<dyn Duplex> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_write(context, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_shutdown(context)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_write_vectored(context, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        (**self).is_write_vectored()
    }
}

pub async fn splice(a: TcpStream, b: TcpStream, idle: Duration) -> std::io::Result<()> {
    a.set_nodelay(true)?;
    b.set_nodelay(true)?;
    pipe(a, b, idle).await;
    Ok(())
}

pub async fn pipe(a: impl AsyncRead + AsyncWrite, b: impl AsyncRead + AsyncWrite, idle: Duration) {
    let start = nanocode_platform::now();
    let last = AtomicU64::new(0);
    let quiet = || start + Duration::from_millis(last.load(Ordering::Relaxed)) + idle;
    let (mut ar, mut aw) = split(a);
    let (mut br, mut bw) = split(b);
    let mut copy = pin!(join(
        half(&mut ar, &mut bw, start, &last),
        half(&mut br, &mut aw, start, &last),
    ));
    loop {
        match select(copy.as_mut(), pin!(sleep_until(quiet()))).await {
            Either::Left(_) => return,
            Either::Right(_) if nanocode_platform::now() >= quiet() => return,
            Either::Right(_) => {}
        }
    }
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

async fn half(
    from: &mut (impl AsyncRead + Unpin),
    to: &mut (impl AsyncWrite + Unpin),
    start: Instant,
    last: &AtomicU64,
) {
    let mut buffer = vec![0; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        if to.write_all(&buffer[..read]).await.is_err() {
            break;
        }
        last.fetch_max(millis(start.elapsed()), Ordering::Relaxed);
    }
    drop(to.shutdown().await);
}
