use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use crate::reactor::{Armed, reactor};

const FAR_FUTURE: Duration = Duration::from_secs(86_400 * 365 * 30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("deadline has elapsed")
    }
}

impl std::error::Error for Elapsed {}

impl From<Elapsed> for io::Error {
    fn from(_: Elapsed) -> Self {
        Self::from(io::ErrorKind::TimedOut)
    }
}

enum Arming {
    Unarmed,
    Armed(Armed),
    Fired,
}

pub struct Sleep {
    deadline: Instant,
    arming: Arming,
}

impl Sleep {
    #[must_use]
    pub const fn until(deadline: Instant) -> Self {
        Self {
            deadline,
            arming: Arming::Unarmed,
        }
    }

    #[must_use]
    pub fn after(duration: Duration) -> Self {
        Self::until(far_future_or(Instant::now(), duration))
    }

    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    #[must_use]
    pub const fn is_elapsed(&self) -> bool {
        matches!(self.arming, Arming::Fired)
    }

    pub fn reset(&mut self, deadline: Instant) {
        self.deadline = deadline;
        self.arming = Arming::Unarmed;
    }
}

fn far_future_or(now: Instant, duration: Duration) -> Instant {
    now.checked_add(duration)
        .unwrap_or_else(|| now + FAR_FUTURE)
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let sleep = self.get_mut();
        loop {
            match &sleep.arming {
                Arming::Fired => return Poll::Ready(()),
                Arming::Armed(armed) => {
                    if armed.poll_fired(context).is_ready() {
                        sleep.arming = Arming::Fired;
                        return Poll::Ready(());
                    }
                    return Poll::Pending;
                }
                Arming::Unarmed => {
                    if sleep.deadline <= Instant::now() {
                        sleep.arming = Arming::Fired;
                        return Poll::Ready(());
                    }
                    let reactor = match reactor() {
                        Ok(reactor) => reactor,
                        Err(error) => panic!("a timer could not reach the reactor: {error}"),
                    };
                    sleep.arming = Arming::Armed(reactor.arm(sleep.deadline));
                }
            }
        }
    }
}

#[must_use]
pub fn sleep(duration: Duration) -> Sleep {
    Sleep::after(duration)
}

#[must_use]
pub const fn sleep_until(deadline: Instant) -> Sleep {
    Sleep::until(deadline)
}

pub fn timeout<F: Future>(
    duration: Duration,
    future: F,
) -> impl Future<Output = Result<F::Output, Elapsed>> {
    timeout_at(far_future_or(Instant::now(), duration), future)
}

pub async fn timeout_at<F: Future>(deadline: Instant, future: F) -> Result<F::Output, Elapsed> {
    let mut future = std::pin::pin!(future);
    let mut delay = Sleep::until(deadline);
    std::future::poll_fn(|context| {
        if let Poll::Ready(output) = future.as_mut().poll(context) {
            return Poll::Ready(Ok(output));
        }
        Pin::new(&mut delay).poll(context).map(|()| Err(Elapsed))
    })
    .await
}
