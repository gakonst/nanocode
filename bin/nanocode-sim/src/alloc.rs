use std::{alloc::Layout, collections::BTreeMap, ops::Range, process::ExitCode};

use nanocode_sys::alloc::{Allocation, Local, is_small};

use crate::rng::{FNV_OFFSET, PPM, Rng, fnv};

const THREADS: usize = 4;
const LIVE_MAX: usize = 384;
const PATTERN_WHOLE_BYTES_MAX: usize = 256;
const PATTERN_HEAD_BYTES: usize = 64;
const PATTERN_STRIDE_BYTES: usize = 251;
const ALLOCATE_PPM: u64 = 400_000;
const FREE_PPM: u64 = 300_000;
const REALLOCATE_PPM: u64 = 220_000;
const EXIT_PPM: u64 = 40_000;
const GROW_PPM: u64 = PPM / 2;
const SMALL_SIZE_PPM: u64 = 700_000;
const MEDIUM_SIZE_PPM: u64 = 930_000;
const SMALL_SIZE_BYTES_MAX: u64 = 1024;
const MEDIUM_SIZE_BYTES_MAX: u64 = 300 << 10;
const LARGE_SIZE_BYTES_MAX: u64 = 2 << 20;
const PLAIN_ALIGN_PPM: u64 = 900_000;
const PLAIN_ALIGN_BYTES: usize = 8;
const ALIGN_SHIFT_MIN: u64 = 4;
const ALIGN_SHIFT_MAX: u64 = 20;
const MUTATION_AFTER_OPS: u64 = 64;
const OVERSTATED_BYTES: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocMutation {
    None,
    OverstateSize,
    ForgetFree,
    Scribble,
}

impl AllocMutation {
    const ALL: [Self; 4] = [
        Self::None,
        Self::OverstateSize,
        Self::ForgetFree,
        Self::Scribble,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mutation| mutation.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::OverstateSize => "overstate_size",
            Self::ForgetFree => "forget_free",
            Self::Scribble => "scribble",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AllocStats {
    pub seeds: u64,
    pub allocations: u64,
    pub frees_local: u64,
    pub frees_remote: u64,
    pub reallocations: u64,
    pub exits: u64,
    pub adopted: u64,
    pub checkpoints: u64,
}

impl AllocStats {
    pub const fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.allocations += other.allocations;
        self.frees_local += other.frees_local;
        self.frees_remote += other.frees_remote;
        self.reallocations += other.reallocations;
        self.exits += other.exits;
        self.adopted += other.adopted;
        self.checkpoints += other.checkpoints;
    }
}

struct Block {
    allocation: Allocation,
    claimed_bytes: usize,
    tag: u64,
    thread: usize,
}

const fn expected(tag: u64, at: usize) -> u8 {
    ((tag >> ((at % 8) * 8)) as u8) ^ (at as u8)
}

fn positions(size: usize, upto: usize) -> impl Iterator<Item = usize> {
    let whole = size <= PATTERN_WHOLE_BYTES_MAX;
    (0..upto.min(size)).filter(move |at| {
        whole || *at < PATTERN_HEAD_BYTES || at % PATTERN_STRIDE_BYTES == 0 || *at + 1 == size
    })
}

impl Block {
    fn small(&self) -> bool {
        is_small(self.allocation.layout())
    }

    fn fill(&mut self) {
        let tag = self.tag;
        let bytes = self.allocation.bytes_mut();
        for at in positions(bytes.len(), bytes.len()) {
            bytes[at] = expected(tag, at);
        }
    }

    fn check(&self, upto: usize) -> Result<(), String> {
        self.check_written(self.allocation.layout().size(), upto)
    }

    fn check_written(&self, written_bytes: usize, upto: usize) -> Result<(), String> {
        let bytes = self.allocation.bytes();
        for at in positions(written_bytes, upto.min(bytes.len())) {
            if bytes[at] != expected(self.tag, at) {
                return Err(format!(
                    "contents: byte {at} of a {}-byte block holds {:#04x}, written {:#04x}",
                    bytes.len(),
                    bytes[at],
                    expected(self.tag, at)
                ));
            }
        }
        Ok(())
    }
}

struct World {
    rng: Rng,
    locals: Vec<Local>,
    live: Vec<Block>,
    by_address: BTreeMap<usize, usize>,
    forgotten: Vec<Block>,
    rejected: Vec<Allocation>,
    mutation: AllocMutation,
    mutated: bool,
    ops_done: u64,
    stats: AllocStats,
    digest: u64,
}

pub struct Outcome {
    pub stats: AllocStats,
    pub wrong: Option<String>,
    pub shape: String,
    pub digest: u64,
}

impl World {
    fn new(seed: u64, mutation: AllocMutation) -> Self {
        Self {
            rng: Rng::derive(seed, "alloc"),
            locals: (0..THREADS).map(|_| Local::new()).collect(),
            live: Vec::new(),
            by_address: BTreeMap::new(),
            forgotten: Vec::new(),
            rejected: Vec::new(),
            mutation,
            mutated: false,
            ops_done: 0,
            stats: AllocStats {
                seeds: 1,
                ..AllocStats::default()
            },
            digest: FNV_OFFSET,
        }
    }

    fn note(&mut self, what: &str, value: u64) {
        self.digest = fnv(self.digest, what.as_bytes());
        self.digest = fnv(self.digest, &value.to_le_bytes());
    }

    fn mutation_due(&mut self, mutation: AllocMutation) -> bool {
        let due = self.mutation == mutation && !self.mutated && self.ops_done >= MUTATION_AFTER_OPS;
        if due {
            self.mutated = true;
        }
        due
    }

    fn layout(&mut self) -> Layout {
        let roll = self.rng.below(PPM);
        let size = if roll < SMALL_SIZE_PPM {
            self.rng.range(1, SMALL_SIZE_BYTES_MAX)
        } else if roll < MEDIUM_SIZE_PPM {
            self.rng.range(1, MEDIUM_SIZE_BYTES_MAX)
        } else {
            self.rng.range(1, LARGE_SIZE_BYTES_MAX)
        };
        let align = if self.rng.chance(PLAIN_ALIGN_PPM) {
            PLAIN_ALIGN_BYTES
        } else {
            1 << self.rng.range(ALIGN_SHIFT_MIN, ALIGN_SHIFT_MAX)
        };
        self.note("layout", size ^ ((align as u64) << 32));
        Layout::from_size_align(
            usize::try_from(size).expect("a simulated size fits in usize"),
            align,
        )
        .expect("a simulated layout is valid")
    }

    fn placed(&self, address: usize, bytes: usize, align: usize) -> Result<(), String> {
        if address % align != 0 {
            return Err(format!(
                "alignment: a block aligned to {align} starts {} bytes past it",
                address % align
            ));
        }
        if let Some((&before, &index)) = self.by_address.range(..=address).next_back()
            && before + self.live[index].claimed_bytes > address
        {
            return Err(format!(
                "handed out twice: a new {bytes}-byte block overlaps a live {}-byte block",
                self.live[index].claimed_bytes
            ));
        }
        if let Some((&after, &index)) = self.by_address.range(address..).next()
            && after < address + bytes
        {
            return Err(format!(
                "handed out twice: a new {bytes}-byte block overlaps a live {}-byte block",
                self.live[index].claimed_bytes
            ));
        }
        Ok(())
    }

    fn claim(&mut self, allocation: Allocation, thread: usize) -> Result<(), String> {
        let layout = allocation.layout();
        if let Err(message) = self.placed(allocation.address(), layout.size(), layout.align()) {
            self.rejected.push(allocation);
            return Err(message);
        }
        let claimed_bytes = if self.mutation_due(AllocMutation::OverstateSize) {
            layout.size() + OVERSTATED_BYTES
        } else {
            layout.size()
        };
        let tag = self.rng.draw();
        let mut block = Block {
            allocation,
            claimed_bytes,
            tag,
            thread,
        };
        block.fill();
        self.by_address
            .insert(block.allocation.address(), self.live.len());
        self.live.push(block);
        Ok(())
    }

    fn remove(&mut self, index: usize) -> Block {
        let block = self.live.swap_remove(index);
        self.by_address.remove(&block.allocation.address());
        if index < self.live.len() {
            self.by_address
                .insert(self.live[index].allocation.address(), index);
        }
        block
    }

    fn allocate(&mut self, thread: usize) -> Result<(), String> {
        let layout = self.layout();
        self.stats.allocations += 1;
        let Some(allocation) = self.locals[thread].allocate(layout) else {
            return Err(format!(
                "the allocator refused {} bytes aligned to {}",
                layout.size(),
                layout.align()
            ));
        };
        self.claim(allocation, thread)
    }

    fn free(&mut self, thread: usize, index: usize) -> Result<(), String> {
        self.live[index].check(usize::MAX)?;
        let block = self.remove(index);
        if block.small() && self.mutation_due(AllocMutation::ForgetFree) {
            self.forgotten.push(block);
            return Ok(());
        }
        if block.thread == thread {
            self.stats.frees_local += 1;
        } else {
            self.stats.frees_remote += 1;
        }
        self.locals[thread].deallocate(block.allocation);
        Ok(())
    }

    fn reallocate(&mut self, thread: usize, index: usize) -> Result<(), String> {
        self.live[index].check(usize::MAX)?;
        let old = self.remove(index);
        let size = old.allocation.layout().size();
        let size_new = if self.rng.chance(GROW_PPM) {
            size + 1 + self.rng.pick(size * 2 + PLAIN_ALIGN_BYTES)
        } else {
            1 + self.rng.pick(size)
        };
        self.note("reallocate", size_new as u64);
        self.stats.reallocations += 1;
        let tag = old.tag;
        let allocation = match self.locals[thread].reallocate(old.allocation, size_new) {
            Ok(allocation) => allocation,
            Err(allocation) => {
                self.rejected.push(allocation);
                return Err(format!(
                    "the allocator refused to resize {size} bytes to {size_new}"
                ));
            }
        };
        let moved = Block {
            allocation,
            claimed_bytes: size_new,
            tag,
            thread,
        };
        if let Err(message) = moved.check_written(size, size.min(size_new)) {
            self.rejected.push(moved.allocation);
            return Err(message);
        }
        self.claim(moved.allocation, thread)
    }

    fn checkpoint(&mut self, thread: usize) -> Result<(), String> {
        self.stats.adopted += self.locals[thread].adopt_abandoned() as u64;
        self.stats.checkpoints += 1;
        let used: usize = self
            .locals
            .iter()
            .map(|local| local.census().blocks_used)
            .sum();
        let tracked = self.live.iter().filter(|block| block.small()).count();
        if used != tracked {
            return Err(format!(
                "census: the heaps hold {used} small blocks, the simulation tracks {tracked} live"
            ));
        }
        Ok(())
    }

    fn step(&mut self) -> Result<(), String> {
        let thread = self.rng.pick(THREADS);
        if !self.live.is_empty() && self.mutation_due(AllocMutation::Scribble) {
            let index = self.rng.pick(self.live.len());
            let block = &mut self.live[index];
            let wrong = !expected(block.tag, 0);
            if let Some(first) = block.allocation.bytes_mut().first_mut() {
                *first = wrong;
            }
        }
        let roll = self.rng.below(PPM);
        self.note("step", roll ^ ((thread as u64) << 40));
        let freeing = (ALLOCATE_PPM..ALLOCATE_PPM + FREE_PPM).contains(&roll);
        if self.live.len() >= LIVE_MAX || (freeing && !self.live.is_empty()) {
            let index = self.rng.pick(self.live.len());
            return self.free(thread, index);
        }
        if roll < ALLOCATE_PPM || self.live.is_empty() {
            return self.allocate(thread);
        }
        if roll < ALLOCATE_PPM + FREE_PPM + REALLOCATE_PPM {
            let index = self.rng.pick(self.live.len());
            return self.reallocate(thread, index);
        }
        if roll < ALLOCATE_PPM + FREE_PPM + REALLOCATE_PPM + EXIT_PPM {
            self.stats.exits += 1;
            self.locals[thread].abandon();
            return Ok(());
        }
        self.checkpoint(thread)
    }

    fn finish(&mut self) -> Result<(), String> {
        self.checkpoint(0)?;
        for block in &self.live {
            block.check(usize::MAX)?;
        }
        self.release();
        self.checkpoint(0)
    }

    fn release(&mut self) {
        self.by_address.clear();
        let blocks: Vec<Block> = self
            .live
            .drain(..)
            .chain(self.forgotten.drain(..))
            .collect();
        for block in blocks {
            self.locals[0].deallocate(block.allocation);
        }
        for allocation in std::mem::take(&mut self.rejected) {
            self.locals[0].deallocate(allocation);
        }
    }
}

pub fn run_seed(seed: u64, ops: u64, mutation: AllocMutation) -> Outcome {
    let mut world = World::new(seed, mutation);
    let mut wrong = None;
    for _ in 0..ops {
        if let Err(message) = world.step() {
            wrong = Some(message);
            break;
        }
        world.ops_done += 1;
    }
    if wrong.is_none() {
        wrong = world.finish().err();
    }
    let shape = format!(
        "ops={} live={} exits={}",
        world.ops_done,
        world.live.len(),
        world.stats.exits
    );
    world.release();
    Outcome {
        stats: world.stats,
        wrong,
        shape,
        digest: world.digest,
    }
}

pub fn judge(
    seed: u64,
    ops: u64,
    mutation: AllocMutation,
    twice: bool,
    verbose: bool,
    totals: &mut AllocStats,
) -> Option<String> {
    let first = run_seed(seed, ops, mutation);
    totals.add(&first.stats);
    if verbose {
        println!("seed {seed} digest {:016x} {}", first.digest, first.shape);
    }
    if first.wrong.is_some() || !twice {
        return first
            .wrong
            .map(|message| format!("{} :: {message}", first.shape));
    }
    let second = run_seed(seed, ops, mutation);
    (second.digest != first.digest || second.wrong.is_some()).then(|| {
        format!(
            "{} :: nondeterminism: trace digest {:016x} then {:016x}",
            first.shape, first.digest, second.digest
        )
    })
}

pub fn replay(seed: u64, ops: u64, mutation: AllocMutation) -> String {
    let mut command = format!("nanocode-sim --scenario alloc --seed {seed} --ops {ops}");
    if mutation != AllocMutation::None {
        command.push_str(" --mutation ");
        command.push_str(mutation.name());
    }
    command
}

pub fn report(
    seeds: &Range<u64>,
    passed: u64,
    failed: u64,
    twice: bool,
    mutation: AllocMutation,
    totals: &AllocStats,
) -> ExitCode {
    println!(
        "RESULT seeds={}..{} scenario=alloc mutation={} passed={passed} failed={failed} twice={twice} stats={totals:?}",
        seeds.start,
        seeds.end,
        mutation.name()
    );
    match mutation {
        AllocMutation::None if failed == 0 => ExitCode::SUCCESS,
        AllocMutation::None => ExitCode::FAILURE,
        _ if failed > 0 => {
            println!("MUTATION {} caught by {failed} seeds", mutation.name());
            ExitCode::SUCCESS
        }
        _ => {
            println!("MUTATION {} NOT caught", mutation.name());
            ExitCode::FAILURE
        }
    }
}
