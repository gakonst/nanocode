use std::{
    future::Future,
    pin::Pin,
    sync::OnceLock,
    task::Poll,
    time::{Duration, Instant, SystemTime},
};

pub type HostFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub trait Host: Send + Sync + 'static {
    fn now(&self) -> Instant;
    fn system_time(&self) -> SystemTime;
    fn sleep(&self, duration: Duration) -> HostFuture;
    fn fill_random(&self, bytes: &mut [u8]) -> std::io::Result<()>;
    fn spawn(&self, future: HostFuture);
    fn uuid_stamps(&self) -> Option<&std::sync::atomic::AtomicU64> {
        None
    }
}

static HOST: OnceLock<&'static dyn Host> = OnceLock::new();

pub fn install(host: &'static dyn Host) -> std::io::Result<()> {
    HOST.set(host)
        .map_err(|_| std::io::Error::other("a platform host is already installed"))
}

pub fn host() -> Option<&'static dyn Host> {
    HOST.get().copied()
}

pub fn now() -> Instant {
    host().map_or_else(Instant::now, |host| host.now())
}

pub fn system_time() -> SystemTime {
    host().map_or_else(SystemTime::now, |host| host.system_time())
}

enum Delay {
    Tokio(tokio::time::Sleep),
    Host(HostFuture),
}

pub fn sleep(duration: Duration) -> impl Future<Output = ()> + Send + 'static {
    let delay = match host() {
        Some(host) => Delay::Host(host.sleep(duration)),
        None => Delay::Tokio(tokio::time::sleep(duration)),
    };
    async move {
        match delay {
            Delay::Tokio(delay) => delay.await,
            Delay::Host(delay) => delay.await,
        }
    }
}

enum Timed<F: Future> {
    Tokio(tokio::time::Timeout<F>),
    Host(F, HostFuture),
}

pub fn timeout<F: Future>(
    duration: Duration,
    future: F,
) -> impl Future<Output = Result<F::Output, crate::Elapsed>> {
    let timed = match host() {
        Some(host) => Timed::Host(future, host.sleep(duration)),
        None => Timed::Tokio(tokio::time::timeout(duration, future)),
    };
    async move {
        match timed {
            Timed::Tokio(timed) => timed.await.map_err(|_| crate::Elapsed),
            Timed::Host(future, mut delay) => {
                let mut future = std::pin::pin!(future);
                std::future::poll_fn(|context| {
                    if let Poll::Ready(output) = future.as_mut().poll(context) {
                        return Poll::Ready(Ok(output));
                    }
                    delay.as_mut().poll(context).map(|()| Err(crate::Elapsed))
                })
                .await
            }
        }
    }
}

pub fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    match host() {
        Some(host) => host.spawn(Box::pin(future)),
        None => drop(tokio::spawn(future)),
    }
}
