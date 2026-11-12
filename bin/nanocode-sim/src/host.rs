use std::{
    cell::RefCell,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nanocode_platform::{Host, HostFuture};

use crate::{executor::Clock, rng::Rng};

pub const SIMULATED_START_UNIX_SECS: u64 = 1_794_441_600;

struct Simulation {
    clock: Arc<Clock>,
    random: Rng,
    spawned: Vec<HostFuture>,
    started: Instant,
}

thread_local! {
    static CURRENT: RefCell<Option<Simulation>> = const { RefCell::new(None) };
}

fn with<R>(use_simulation: impl FnOnce(&mut Simulation) -> R) -> R {
    CURRENT.with_borrow_mut(|current| {
        use_simulation(
            current
                .as_mut()
                .expect("the platform host is used only inside a simulated seed"),
        )
    })
}

struct SimulatedHost;

static HOST: SimulatedHost = SimulatedHost;

impl Host for SimulatedHost {
    fn now(&self) -> Instant {
        with(|simulation| simulation.started + Duration::from_nanos(simulation.clock.now_ns()))
    }

    fn system_time(&self) -> SystemTime {
        let now_ns = with(|simulation| simulation.clock.now_ns());
        UNIX_EPOCH + Duration::from_secs(SIMULATED_START_UNIX_SECS) + Duration::from_nanos(now_ns)
    }

    fn sleep(&self, duration: Duration) -> HostFuture {
        with(|simulation| Box::pin(simulation.clock.sleep(duration)))
    }

    fn fill_random(&self, bytes: &mut [u8]) -> std::io::Result<()> {
        with(|simulation| {
            for chunk in bytes.chunks_mut(size_of::<u64>()) {
                let drawn = simulation.random.draw().to_le_bytes();
                chunk.copy_from_slice(&drawn[..chunk.len()]);
            }
        });
        Ok(())
    }

    fn spawn(&self, future: HostFuture) {
        with(|simulation| simulation.spawned.push(future));
    }
}

pub fn install() -> std::io::Result<()> {
    nanocode_platform::install(&HOST)
}

pub fn enter(seed: u64, clock: &Arc<Clock>) {
    CURRENT.with_borrow_mut(|current| {
        *current = Some(Simulation {
            clock: Arc::clone(clock),
            random: Rng::derive(seed, "host"),
            spawned: Vec::new(),
            started: Instant::now(),
        });
    });
}

pub fn spawned() -> Vec<HostFuture> {
    CURRENT.with_borrow_mut(|current| {
        current
            .as_mut()
            .map(|simulation| std::mem::take(&mut simulation.spawned))
            .unwrap_or_default()
    })
}
