#[cfg(target_family = "wasm")]
mod host;
#[cfg(target_family = "wasm")]
pub use host::{Host, Instant, LocalFuture, install, spawn_local};

#[cfg(unix)]
#[allow(unsafe_code)]
pub mod unix;
mod zone;
pub use zone::{local_date, time_zone_name, utc_date, utc_offset_seconds};

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub use native::{Host, HostFuture, host, install, now, sleep, spawn, system_time, timeout};

#[cfg(not(target_family = "wasm"))]
pub use std::time::Instant;
pub use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_family = "wasm")]
pub fn system_time() -> SystemTime {
    host::current().system_time()
}

#[cfg(target_family = "wasm")]
pub fn sleep(duration: Duration) -> LocalFuture<'static, ()> {
    host::current().sleep(duration)
}

#[cfg(target_family = "wasm")]
pub fn now() -> Instant {
    Instant::now()
}

#[cfg(target_family = "wasm")]
pub fn spawn(future: impl std::future::Future<Output = ()> + 'static) {
    spawn_local(future);
}

#[derive(Debug, Eq, PartialEq)]
pub struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("deadline has elapsed")
    }
}

impl std::error::Error for Elapsed {}

impl From<Elapsed> for std::io::Error {
    fn from(elapsed: Elapsed) -> Self {
        Self::new(std::io::ErrorKind::TimedOut, elapsed)
    }
}

#[cfg(target_family = "wasm")]
pub async fn timeout<F: std::future::Future>(
    duration: Duration,
    future: F,
) -> Result<F::Output, Elapsed> {
    let mut future = std::pin::pin!(future);
    let mut delay = sleep(duration);
    std::future::poll_fn(|context| {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(context) {
            return std::task::Poll::Ready(Ok(output));
        }
        delay.as_mut().poll(context).map(|()| Err(Elapsed))
    })
    .await
}

pub fn unix_millis() -> u64 {
    system_time()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(not(target_family = "wasm"))]
const KERNEL_ENTROPY_READ: usize = 256;

#[cfg(not(target_family = "wasm"))]
struct Entropy {
    bytes: [u8; KERNEL_ENTROPY_READ],
    unread: usize,
}

#[cfg(not(target_family = "wasm"))]
thread_local! {
    static ENTROPY: std::cell::RefCell<Entropy> = const {
        std::cell::RefCell::new(Entropy {
            bytes: [0; KERNEL_ENTROPY_READ],
            unread: 0,
        })
    };
}

#[cfg(not(target_family = "wasm"))]
fn kernel_random(bytes: &mut [u8]) -> std::io::Result<()> {
    getrandom::fill(bytes).map_err(std::io::Error::other)
}

#[cfg(not(target_family = "wasm"))]
pub fn fill_random(bytes: &mut [u8]) -> std::io::Result<()> {
    if let Some(host) = host() {
        return host.fill_random(bytes);
    }
    if bytes.len() >= KERNEL_ENTROPY_READ {
        return kernel_random(bytes);
    }
    let buffered = ENTROPY.try_with(|entropy| {
        let mut entropy = entropy.borrow_mut();
        if entropy.unread < bytes.len() {
            kernel_random(&mut entropy.bytes)?;
            entropy.unread = KERNEL_ENTROPY_READ;
        }
        let start = entropy.unread - bytes.len();
        let taken = &mut entropy.bytes[start..start + bytes.len()];
        bytes.copy_from_slice(taken);
        taken.fill(0);
        entropy.unread = start;
        Ok(())
    });
    match buffered {
        Ok(filled) => filled,
        Err(_) => kernel_random(bytes),
    }
}

#[cfg(target_family = "wasm")]
pub fn fill_random(bytes: &mut [u8]) -> std::io::Result<()> {
    host::current().fill_random(bytes)
}

pub fn uuid_v4() -> uuid::Uuid {
    let mut bytes = [0; 16];
    fill_random(&mut bytes).expect("the host could not provide UUID randomness");
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

#[cfg(not(target_family = "wasm"))]
pub fn uuid_v7() -> uuid::Uuid {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let millis = system_time()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock predates the Unix epoch")
        .as_millis();
    assert!(
        millis < 1 << 48,
        "the system clock exceeds the UUIDv7 timestamp range"
    );
    let floor = (millis as u64) << 12;
    let next = |last: u64| floor.max(last + 1);
    let stamp = next(
        LAST.try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            Some(next(last))
        })
        .unwrap_or_else(|last| last),
    );
    let [.., high, low] = stamp.to_be_bytes();
    let mut bytes = [0; 10];
    bytes[0] = high & 0x0f;
    bytes[1] = low;
    fill_random(&mut bytes[2..]).expect("the system could not provide UUID randomness");
    uuid::Builder::from_unix_timestamp_millis(stamp >> 12, &bytes).into_uuid()
}

#[cfg(target_family = "wasm")]
pub fn uuid_v7() -> uuid::Uuid {
    let millis = system_time()
        .duration_since(UNIX_EPOCH)
        .expect("the host clock predates the Unix epoch")
        .as_millis();
    assert!(
        millis < 1 << 48,
        "the host clock exceeds the UUIDv7 timestamp range"
    );
    let mut bytes = [0; 10];
    fill_random(&mut bytes).expect("the host could not provide UUID randomness");
    uuid::Builder::from_unix_timestamp_millis(millis as u64, &bytes).into_uuid()
}

#[cfg(not(target_family = "wasm"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_family = "wasm"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_family = "wasm")]
pub trait MaybeSend {}
#[cfg(target_family = "wasm")]
impl<T> MaybeSend for T {}

#[cfg(not(target_family = "wasm"))]
pub trait MaybeSync: Sync {}
#[cfg(not(target_family = "wasm"))]
impl<T: Sync> MaybeSync for T {}
#[cfg(target_family = "wasm")]
pub trait MaybeSync {}
#[cfg(target_family = "wasm")]
impl<T> MaybeSync for T {}
