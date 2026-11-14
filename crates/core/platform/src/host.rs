use std::{
    cell::RefCell,
    future::Future,
    ops::{Add, AddAssign, Sub, SubAssign},
    pin::Pin,
    rc::Rc,
    time::{Duration, SystemTime},
};

pub type LocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub trait Host: 'static {
    fn spawn_local(&self, future: LocalFuture<'static, ()>);
    fn sleep(&self, duration: Duration) -> LocalFuture<'static, ()>;
    fn monotonic_time(&self) -> Duration;
    fn system_time(&self) -> SystemTime;
    fn fill_random(&self, bytes: &mut [u8]) -> std::io::Result<()>;
}

thread_local! {
    static HOST: RefCell<Option<Rc<dyn Host>>> = const { RefCell::new(None) };
}

pub fn install(host: Rc<dyn Host>) -> std::io::Result<()> {
    HOST.with_borrow_mut(|current| {
        if current.is_some() {
            return Err(std::io::Error::other(
                "a platform host is already installed",
            ));
        }
        *current = Some(host);
        Ok(())
    })
}

pub(super) fn current() -> Rc<dyn Host> {
    HOST.with_borrow(|host| {
        host.clone()
            .expect("a Rust platform host must be installed before using portable runtime services")
    })
}

pub fn sleep_until(deadline: Instant) -> LocalFuture<'static, ()> {
    current().sleep(deadline.saturating_duration_since(Instant::now()))
}

pub struct Sleep {
    deadline: Instant,
    wake: LocalFuture<'static, ()>,
}

impl Sleep {
    pub fn after(duration: Duration) -> Self {
        Self::until(Instant::now() + duration)
    }

    pub fn until(deadline: Instant) -> Self {
        Self {
            deadline,
            wake: sleep_until(deadline),
        }
    }

    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn reset(&mut self, deadline: Instant) {
        *self = Self::until(deadline);
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        self.get_mut().wake.as_mut().poll(context)
    }
}

pub fn spawn_local(future: impl Future<Output = ()> + 'static) {
    current().spawn_local(Box::pin(future));
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Instant(Duration);

impl Instant {
    pub fn now() -> Self {
        Self(current().monotonic_time())
    }

    pub fn elapsed(&self) -> Duration {
        Self::now().duration_since(*self)
    }

    #[allow(clippy::missing_const_for_fn)]
    pub fn duration_since(&self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }

    #[allow(clippy::missing_const_for_fn)]
    pub fn saturating_duration_since(&self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }

    #[allow(clippy::missing_const_for_fn)]
    pub fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
        self.0.checked_sub(earlier.0)
    }

    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration).map(Self)
    }

    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration).map(Self)
    }
}

impl Add<Duration> for Instant {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self {
        Self(self.0 + rhs)
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) {
        self.0 += rhs;
    }
}

impl Sub<Duration> for Instant {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self {
        Self(self.0 - rhs)
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, rhs: Duration) {
        self.0 -= rhs;
    }
}

impl Sub for Instant {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Duration {
        self.duration_since(rhs)
    }
}
