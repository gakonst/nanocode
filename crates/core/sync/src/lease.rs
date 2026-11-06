use futures_util::future::BoxFuture;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::{spawn, time::sleep};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lease {
    Held,
    Reconnecting,
    Released,
    Lapsed,
    GivenUp,
}

impl Lease {
    #[must_use]
    pub const fn next(self, to: Self) -> Option<Self> {
        match self {
            Self::Held | Self::Reconnecting => Some(to),
            Self::Released | Self::Lapsed | Self::GivenUp => None,
        }
    }

    #[must_use]
    pub const fn ended(self) -> bool {
        matches!(self, Self::Released | Self::Lapsed | Self::GivenUp)
    }
}

pub trait Renew: Send + 'static {
    fn renew(&mut self) -> BoxFuture<'_, Lease>;

    fn moved(&mut self, lease: Lease);
}

#[derive(Clone, Copy, Debug)]
pub struct Keeping {
    pub every: Duration,
    pub reconnecting: Duration,
    pub watch: Duration,
    pub within: Duration,
}

pub fn keep_lease<R: Renew>(
    mut renew: R,
    keeping: Keeping,
    released: impl Fn() -> bool + Send + 'static,
    expired: impl FnOnce() + Send + 'static,
) {
    let started = Instant::now();
    let renewed = Arc::new(AtomicU64::new(0));
    let ended = Arc::new(AtomicBool::new(false));
    let (watched, stopped) = (Arc::clone(&renewed), Arc::clone(&ended));
    thread::spawn(move || {
        loop {
            thread::sleep(keeping.watch);
            if released() || stopped.load(Ordering::SeqCst) {
                return;
            }
            let since = started
                .elapsed()
                .as_millis()
                .saturating_sub(u128::from(watched.load(Ordering::SeqCst)));
            if since > keeping.within.as_millis() {
                expired();
                return;
            }
        }
    });
    let ended = Ended(ended);
    spawn(async move {
        let _ended = ended;
        let mut lease = Lease::Held;
        loop {
            sleep(if lease == Lease::Reconnecting {
                keeping.reconnecting
            } else {
                keeping.every
            })
            .await;
            let found = renew.renew().await;
            let Some(next) = lease.next(found) else {
                return;
            };
            lease = next;
            if lease == Lease::Held {
                renewed.store(
                    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    Ordering::SeqCst,
                );
            }
            renew.moved(lease);
            if lease.ended() {
                return;
            }
        }
    });
}

struct Ended(Arc<AtomicBool>);

impl Drop for Ended {
    fn drop(&mut self) {
        if !thread::panicking() {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    until: Instant,
    delay: Duration,
    most: Duration,
}

impl Backoff {
    #[must_use]
    pub fn within(within: Duration, first: Duration, most: Duration) -> Self {
        Self {
            until: Instant::now() + within,
            delay: first,
            most,
        }
    }
}

impl Iterator for Backoff {
    type Item = Duration;

    fn next(&mut self) -> Option<Duration> {
        let last = self.until.checked_sub(self.delay).unwrap_or(self.until);
        if Instant::now() >= last {
            return None;
        }
        let delay = self.delay;
        self.delay = (delay * 2).min(self.most);
        Some(delay)
    }
}

pub async fn retry<T, E, F>(
    mut attempt: impl FnMut() -> F,
    mut again: impl FnMut(&E) -> Option<Duration>,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    loop {
        match attempt().await {
            Err(error) => match again(&error) {
                Some(delay) => sleep(delay).await,
                None => return Err(error),
            },
            done => return done,
        }
    }
}
