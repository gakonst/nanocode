use std::{future::Future, pin::Pin, task::Poll, time::Duration};

use crate::{Instant, Sleep, now};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MissedTickBehavior {
    #[default]
    Burst,
    Delay,
}

const MISSED_TICK_TOLERANCE: Duration = Duration::from_millis(5);

pub struct Interval {
    period: Duration,
    behavior: MissedTickBehavior,
    delay: Sleep,
}

pub fn interval(period: Duration) -> Interval {
    interval_at(now(), period)
}

pub fn interval_at(start: Instant, period: Duration) -> Interval {
    assert!(!period.is_zero(), "an interval's period must be positive");
    Interval {
        period,
        behavior: MissedTickBehavior::Burst,
        delay: Sleep::until(start),
    }
}

impl Interval {
    pub const fn period(&self) -> Duration {
        self.period
    }

    pub const fn missed_tick_behavior(&self) -> MissedTickBehavior {
        self.behavior
    }

    pub const fn set_missed_tick_behavior(&mut self, behavior: MissedTickBehavior) {
        self.behavior = behavior;
    }

    pub fn poll_tick(&mut self, context: &mut std::task::Context<'_>) -> Poll<Instant> {
        if Pin::new(&mut self.delay).poll(context).is_pending() {
            return Poll::Pending;
        }
        let fired = self.delay.deadline();
        let current = now();
        let next = match self.behavior {
            MissedTickBehavior::Delay if current > fired + MISSED_TICK_TOLERANCE => {
                current + self.period
            }
            MissedTickBehavior::Burst | MissedTickBehavior::Delay => fired + self.period,
        };
        self.delay.reset(next);
        Poll::Ready(fired)
    }

    pub async fn tick(&mut self) -> Instant {
        std::future::poll_fn(|context| self.poll_tick(context)).await
    }

    pub fn reset(&mut self) {
        self.delay.reset(now() + self.period);
    }
}
