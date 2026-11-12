use crate::Atom;
use nanocode_sys::AtomicStatic;
use std::{
    fmt,
    panic::Location,
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[cfg(target_family = "wasm")]
use nanocode_platform::Instant;
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;

fn elapsed() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    u64::try_from((Instant::now() - epoch).as_nanos()).unwrap_or(u64::MAX)
}

fn holders() -> &'static Atom<imbl::Vector<Weak<Holder>>> {
    static HOLDERS: OnceLock<Atom<imbl::Vector<Weak<Holder>>>> = OnceLock::new();
    HOLDERS.get_or_init(Atom::default)
}

#[derive(Default)]
pub(crate) struct Holder {
    at: AtomicStatic<Location<'static>>,
    waiters: AtomicUsize,
    waiter: AtomicStatic<Location<'static>>,
    since: AtomicU64,
}

impl Holder {
    pub(crate) fn registered() -> Arc<Self> {
        let holder = Arc::new(Self::default());
        let weak = Arc::downgrade(&holder);
        holders().update(|holders| {
            if holders.len().is_power_of_two() {
                holders.retain(|holder| holder.strong_count() > 0);
            }
            holders.push_back(weak.clone());
        });
        holder
    }

    pub(crate) fn take(&self, at: &'static Location<'static>) {
        self.at.store(Some(at));
    }

    pub(crate) fn release(&self) {
        self.at.store(None);
    }
}

pub(crate) struct Wait<'holder>(&'holder Holder);

impl<'holder> Wait<'holder> {
    pub(crate) fn start(at: &'static Location<'static>, holder: &'holder Holder) -> Self {
        if holder.waiters.fetch_add(1, Ordering::AcqRel) == 0 {
            holder.since.store(elapsed(), Ordering::Release);
            holder.waiter.store(Some(at));
        }
        Self(holder)
    }
}

impl Drop for Wait<'_> {
    fn drop(&mut self) {
        if self.0.waiters.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.waiter.store(None);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SlowWait {
    pub waiter: &'static Location<'static>,
    pub waiters: usize,
    pub waited: Duration,
    pub holder: Option<&'static Location<'static>>,
}

impl fmt::Display for SlowWait {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} and {} more have waited {}s for a lock",
            self.waiter,
            self.waiters.saturating_sub(1),
            self.waited.as_secs()
        )?;
        match self.holder {
            Some(at) => write!(formatter, " that {at} holds"),
            None => formatter.write_str(" nobody holds"),
        }
    }
}

#[must_use]
pub fn slow_waits(longer_than: Duration) -> Vec<SlowWait> {
    let now = elapsed();
    let mut waits: Vec<SlowWait> = holders()
        .get()
        .iter()
        .filter_map(Weak::upgrade)
        .filter_map(|holder| {
            let waiters = holder.waiters.load(Ordering::Acquire);
            let waiter = holder.waiter.load()?;
            let waited =
                Duration::from_nanos(now.saturating_sub(holder.since.load(Ordering::Acquire)));
            (waiters > 0 && waited > longer_than).then(|| SlowWait {
                waiter,
                waiters,
                waited,
                holder: holder.at.load(),
            })
        })
        .collect();
    waits.sort_by_key(|wait| std::cmp::Reverse(wait.waited));
    waits
}
