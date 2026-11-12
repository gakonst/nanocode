use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

use crate::{
    executor::{Clock, Ended, Executor, Task},
    faults::Profile,
    model::{Model, Stats},
    rng::Rng,
    store::{Mutation, SimKv},
    workload::{Proc, World, reopen_check},
};

pub const STATES_MIN: u64 = 1;
pub const STATES_MAX: u64 = 6;
pub const PROCS_MIN: u64 = 1;
pub const PROCS_MAX: u64 = 4;
pub const RECORDS_MAX: u64 = 3;
pub const CRASH_AFTER_NS_MIN: u64 = 500_000;
pub const CRASH_AFTER_NS_MAX: u64 = 200_000_000;
pub const POWER_LOSS_PPM: u64 = 500_000;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub ops: u64,
    pub mutation: Mutation,
    pub slots: bool,
    pub verbose: bool,
}

pub struct Outcome {
    pub digest: u64,
    pub wrong: Option<String>,
    pub stats: Stats,
    pub shape: String,
}

struct Shape {
    states: usize,
    procs: usize,
    records_max: u64,
}

impl Shape {
    fn of(seed: u64) -> Self {
        let mut rng = Rng::derive(seed, "shape");
        let states = rng.range(STATES_MIN, STATES_MAX);
        let procs = rng.range(PROCS_MIN, PROCS_MAX);
        Self {
            states: usize::try_from(states).expect("a state count fits in usize"),
            procs: usize::try_from(procs).expect("a process count fits in usize"),
            records_max: rng.range(0, RECORDS_MAX),
        }
    }
}

struct Run<'w> {
    seed: u64,
    world: &'w World,
    shape: Shape,
    executor: Executor,
    crashes: Rng,
}

impl Run<'_> {
    fn epoch(&mut self, epoch: u64) {
        let world = self.world;
        let mut procs: Vec<Proc> = (0..self.shape.procs)
            .map(|id| Proc::new(&world.kv, self.seed, epoch, id, world.slots))
            .collect();
        let modes: Vec<_> = procs.iter().map(Proc::mode_name).collect();
        world.kv.trace(&format!("epoch {epoch} modes {modes:?}"));
        let crash_at_ns =
            world.clock.now_ns() + self.crashes.range(CRASH_AFTER_NS_MIN, CRASH_AFTER_NS_MAX);
        let tasks: Vec<Task<'_>> = procs
            .iter_mut()
            .map(|proc| Box::pin(proc.run(world)) as Task<'_>)
            .collect();
        let ended = self.executor.run(tasks, Some(crash_at_ns));
        world.model.borrow_mut().stats.epochs += 1;
        if ended == Ended::Stuck {
            world.model.borrow_mut().wrong(format!(
                "epoch {epoch}: every process waits and no timer is set"
            ));
        }
        if world.failed() || ended != Ended::Crashed {
            return;
        }
        let power = self.crashes.chance(POWER_LOSS_PPM);
        world
            .kv
            .trace(&format!("crash epoch {epoch} power_loss={power}"));
        let lost = power.then(|| world.kv.with(crate::store::Kv::power_loss));
        world.model.borrow_mut().crash(lost.as_ref());
        self.check(&format!("e{epoch}"));
    }

    fn check(&mut self, label: &str) {
        let world = self.world;
        let task: Task<'_> = Box::pin(reopen_check(world, label));
        if self.executor.run(vec![task], None) != Ended::Finished {
            world
                .model
                .borrow_mut()
                .wrong(format!("check {label}: the reopened store never answered"));
        }
    }
}

pub fn simulate(seed: u64, options: Options) -> Outcome {
    let profile = Profile::of(seed);
    let shape = Shape::of(seed);
    let describe = format!(
        "profile={} {:?} states={} procs={} records<={}",
        profile.name, profile.durability, shape.states, shape.procs, shape.records_max
    );
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let kv = SimKv::new(
        seed,
        profile.clone(),
        options.mutation,
        options.verbose,
        Arc::clone(&clock),
    );
    kv.trace(&format!("seed {seed} {describe} {profile:?}"));
    let world = World {
        kv: kv.clone(),
        clock: Arc::clone(&clock),
        model: RefCell::new(Model::new(profile.durability, shape.states)),
        ops_left: Cell::new(options.ops),
        states: shape.states,
        records_max: shape.records_max,
        slots: options.slots,
    };
    let mut run = Run {
        seed,
        world: &world,
        shape,
        executor: Executor::new(clock, Rng::derive(seed, "scheduler")),
        crashes: Rng::derive(seed, "crash"),
    };
    let mut epoch = 0;
    while world.ops_left.get() > 0 && !world.failed() {
        run.epoch(epoch);
        epoch += 1;
    }
    if !world.failed() {
        world.model.borrow_mut().crash(None);
        run.check("final");
    }
    drop(run);
    let mut model = world.model.into_inner();
    model.stats.kv_faults = kv.with(|kv| kv.faults);
    Outcome {
        digest: kv.with(|kv| kv.digest),
        wrong: model.wrong,
        stats: model.stats,
        shape: describe,
    }
}

pub fn run_seed(seed: u64, options: Options) -> Outcome {
    std::panic::catch_unwind(|| simulate(seed, options)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        Outcome {
            digest: 0,
            wrong: Some(format!("panic: {message}")),
            stats: Stats::default(),
            shape: String::new(),
        }
    })
}
