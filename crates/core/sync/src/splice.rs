use futures_util::future::{Either, join, select};
use std::{
    pin::pin,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, split},
    net::TcpStream,
    time::{Instant, sleep_until},
};

pub async fn splice(a: TcpStream, b: TcpStream, idle: Duration) -> std::io::Result<()> {
    a.set_nodelay(true)?;
    b.set_nodelay(true)?;
    pipe(a, b, idle).await;
    Ok(())
}

pub async fn pipe(a: impl AsyncRead + AsyncWrite, b: impl AsyncRead + AsyncWrite, idle: Duration) {
    let start = Instant::now();
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
            Either::Right(_) if Instant::now() >= quiet() => return,
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
