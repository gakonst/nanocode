use nanocode_sync::{Semaphore, SemaphorePermit};
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MEMORY_MIB: u64 = 1024;
const MEMORY_EVERY: Duration = Duration::from_millis(100);
const SHELL_EVERY: Duration = Duration::from_millis(25);
const TYPICAL_CALL: u64 = 128 << 20;
pub const CALL_BASE: u64 = 40 << 20;
pub const CALL_PER_BYTE: u64 = 18;
const HIGH_WATER: u64 = 90;
const SAMPLE_MS: u64 = 25;
const UNLIMITED: u64 = 1 << 50;

pub struct Sizes {
    pub limit: Option<u64>,
    pub call: u64,
    pub together: Option<u64>,
    pub permits: usize,
    usage: Option<(PathBuf, u64)>,
    began: Instant,
    sampled: AtomicU64,
    used: AtomicU64,
}

impl Sizes {
    #[must_use]
    pub fn derive(limit: Option<u64>) -> Self {
        let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let call = limit.map_or(MEMORY_MIB << 20, |limit| (MEMORY_MIB << 20).min(limit / 4));
        let together = limit.map(|limit| limit / 3);
        let permits = together.map_or(cpus, |together| {
            cpus.min(
                usize::try_from(together / TYPICAL_CALL)
                    .unwrap_or(usize::MAX)
                    .max(1),
            )
        });
        let usage = limit
            .and_then(|_| usage_file())
            .zip(limit.map(|limit| limit / 100 * HIGH_WATER));
        let used = usage.as_ref().map_or(0, |(file, _)| used(file));
        Self {
            limit,
            call,
            together,
            permits,
            usage,
            began: Instant::now(),
            sampled: AtomicU64::new(0),
            used: AtomicU64::new(used),
        }
    }

    #[must_use]
    pub fn describe(&self) -> String {
        let mib = |bytes: Option<u64>| {
            bytes.map_or_else(|| "none".to_owned(), |b| format!("{} MiB", b >> 20))
        };
        format!(
            "container memory limit {}; a call holds at most {}; calls that reach no host hold at most {} together, run {} at once, and give way past {} in use",
            mib(self.limit),
            mib(Some(self.call)),
            mib(self.together),
            self.permits,
            mib(self.usage.as_ref().map(|(_, high)| *high))
        )
    }

    #[must_use]
    pub fn high(&self) -> bool {
        let Some((file, high)) = &self.usage else {
            return false;
        };
        let now = u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX);
        let seen = self.sampled.load(Ordering::Relaxed);
        if now.saturating_sub(seen) >= SAMPLE_MS
            && self
                .sampled
                .compare_exchange(seen, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            self.used.store(used(file), Ordering::Relaxed);
        }
        self.used.load(Ordering::Relaxed) > *high
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Call,
    Together,
}

#[derive(Clone, Copy, Debug)]
pub struct Stopped {
    pub stop: Stop,
    pub resident: u64,
    pub processes: usize,
    pub most: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    TooLarge { expected: u64, call: u64 },
    Memory { wait: Duration },
    Cpu { wait: Duration },
    Short,
}

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { expected, call } => write!(
                f,
                "The input is too large for this backend's shell: it would hold about {} MiB, more than the {} MiB a call may. Nothing ran; work on part of it at a time.",
                expected >> 20,
                call >> 20
            ),
            Self::Memory { wait } => write!(
                f,
                "The shell is busy: its calls on this backend had all the memory it can spare for {}s. Nothing ran; try again shortly.",
                wait.as_secs()
            ),
            Self::Cpu { wait } => write!(
                f,
                "The shell is busy: every CPU of this backend ran other calls for {}s. Nothing ran; try again shortly.",
                wait.as_secs()
            ),
            Self::Short => f.write_str(
                "The shell is busy: this backend is short of memory. Nothing ran; try again shortly.",
            ),
        }
    }
}

pub struct Admitted<'a> {
    _cpu: SemaphorePermit<'a>,
    _reserved: Option<SemaphorePermit<'a>>,
    expected: u64,
    waited: Duration,
}

impl Admitted<'_> {
    #[must_use]
    pub const fn expected(&self) -> u64 {
        self.expected
    }

    #[must_use]
    pub const fn waited(&self) -> Duration {
        self.waited
    }
}

pub struct Budget {
    sizes: Sizes,
    computing: Semaphore,
    cpu_wait: Duration,
    room: Option<Semaphore>,
    slots: Box<[Slot]>,
    total: AtomicU64,
    calls: AtomicU64,
}

#[derive(Default)]
struct Slot {
    call: AtomicU64,
    reserved: AtomicU64,
    held: AtomicU64,
}

impl Budget {
    #[must_use]
    pub fn new(sizes: Sizes, cpu_wait: Duration) -> Self {
        Self {
            computing: Semaphore::new(sizes.permits),
            cpu_wait,
            room: sizes.together.map(|together| {
                Semaphore::new(usize::try_from(together >> 20).unwrap_or(usize::MAX))
            }),
            slots: (0..sizes.permits.max(1)).map(|_| Slot::default()).collect(),
            sizes,
            total: AtomicU64::new(0),
            calls: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub const fn sizes(&self) -> &Sizes {
        &self.sizes
    }

    pub async fn admit(&self, bytes: usize) -> Result<Admitted<'_>, Busy> {
        let expected = u64::try_from(bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(CALL_PER_BYTE)
            .saturating_add(CALL_BASE);
        if expected > self.sizes.call || self.sizes.together.is_some_and(|t| expected > t) {
            return Err(Busy::TooLarge {
                expected,
                call: self.sizes.call,
            });
        }
        let began = Instant::now();
        let reserved = match &self.room {
            None => None,
            Some(room) => {
                let mib = u32::try_from(expected.div_ceil(1 << 20)).unwrap_or(u32::MAX);
                match nanocode_platform::timeout(self.cpu_wait, room.acquire_many(mib)).await {
                    Ok(Ok(reserved)) => Some(reserved),
                    Ok(Err(_)) => return Err(Busy::Short),
                    Err(_) => {
                        return Err(Busy::Memory {
                            wait: self.cpu_wait,
                        });
                    }
                }
            }
        };
        let left = self.cpu_wait.saturating_sub(began.elapsed());
        let permit = match nanocode_platform::timeout(left, self.computing.acquire()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => return Err(Busy::Short),
            Err(_) => {
                return Err(Busy::Cpu {
                    wait: self.cpu_wait,
                });
            }
        };
        if self.sizes.high() {
            return Err(Busy::Short);
        }
        Ok(Admitted {
            _cpu: permit,
            _reserved: reserved,
            expected,
            waited: began.elapsed(),
        })
    }

    pub fn charge<'a>(
        &'a self,
        held: &mut Vec<SemaphorePermit<'a>>,
        base: u64,
        bytes: usize,
    ) -> Result<(), Busy> {
        let more = u64::try_from(bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(CALL_PER_BYTE);
        let holding: u64 = held
            .iter()
            .map(|permit| permit.num_permits() as u64)
            .sum::<u64>()
            << 20;
        let expected = base.saturating_add(holding).saturating_add(more);
        if expected > self.sizes.call {
            return Err(Busy::TooLarge {
                expected,
                call: self.sizes.call,
            });
        }
        let Some(room) = &self.room else {
            return Ok(());
        };
        let mib = u32::try_from(more.div_ceil(1 << 20)).unwrap_or(u32::MAX);
        let permit = room.try_acquire_many(mib).map_err(|_| Busy::Memory {
            wait: Duration::ZERO,
        })?;
        held.push(permit);
        Ok(())
    }

    #[must_use]
    pub fn stopped(&self, stop: Stop) -> String {
        match stop {
            Stop::Call => format!(
                "The call used more than {} MiB of memory and was stopped: give it less input, or work on part of it at a time.",
                self.sizes.call >> 20
            ),
            Stop::Together => format!(
                "The shell's calls on this backend held more memory together than it can spare (at most {} MiB), and this one, the largest, was stopped: try again shortly, or give it less input.",
                self.sizes.together.unwrap_or_default() >> 20
            ),
        }
    }

    pub async fn watch(&self, pid: Option<u32>, admitted: Option<&Admitted<'_>>) -> Stopped {
        let together = admitted.and_then(|admitted| {
            let budget = self.sizes.together?;
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            Together::new(self, call, budget, admitted.expected)
        });
        watch(pid, self.sizes.call, together).await
    }

    fn picked(&self, weigh: impl Fn(u64, u64) -> Option<u64>) -> Option<u64> {
        self.slots
            .iter()
            .filter_map(|slot| {
                let call = slot.call.load(Ordering::Acquire).checked_sub(1)?;
                let weight = weigh(
                    slot.reserved.load(Ordering::Relaxed),
                    slot.held.load(Ordering::Relaxed),
                )?;
                Some((weight, call))
            })
            .max()
            .map(|(_, call)| call)
    }
}

struct Together<'a> {
    budget: &'a Budget,
    call: u64,
    slot: usize,
    most: u64,
    expected: u64,
}

impl<'a> Together<'a> {
    fn new(budget: &'a Budget, call: u64, most: u64, expected: u64) -> Option<Self> {
        let slot = budget.slots.iter().position(|slot| {
            slot.call
                .compare_exchange(0, call + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        })?;
        budget.slots[slot]
            .reserved
            .store(expected, Ordering::Relaxed);
        budget.slots[slot].held.store(0, Ordering::Relaxed);
        budget.total.fetch_add(expected, Ordering::Relaxed);
        Some(Self {
            budget,
            call,
            slot,
            most,
            expected,
        })
    }

    fn over(&self, resident: u64) -> bool {
        let slot = &self.budget.slots[self.slot];
        let before = slot
            .held
            .swap(resident, Ordering::Relaxed)
            .max(self.expected);
        let after = resident.max(self.expected);
        let total = if after >= before {
            self.budget
                .total
                .fetch_add(after - before, Ordering::Relaxed)
                + (after - before)
        } else {
            self.budget
                .total
                .fetch_sub(before - after, Ordering::Relaxed)
                - (before - after)
        };
        let past = || {
            self.budget
                .picked(|reserved, held| (held > reserved).then(|| held - reserved))
        };
        (total > self.most && past() == Some(self.call))
            || (self.budget.sizes.high()
                && self
                    .budget
                    .picked(|reserved, held| Some(reserved.max(held)))
                    == Some(self.call))
    }
}

impl Drop for Together<'_> {
    fn drop(&mut self) {
        let slot = &self.budget.slots[self.slot];
        let weight = slot.held.load(Ordering::Relaxed).max(self.expected);
        self.budget.total.fetch_sub(weight, Ordering::Relaxed);
        slot.call.store(0, Ordering::Release);
    }
}

#[must_use]
pub fn container_limit() -> Option<u64> {
    let total = read(Path::new("/proc/meminfo")).and_then(|info| {
        info.lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))
            .and_then(|kib| kib.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|kib| kib * 1024)
    });
    let mut limits = Vec::new();
    for (controllers, path) in cgroups() {
        if controllers.is_empty() {
            limits.extend(up("/sys/fs/cgroup", &path, "memory.max"));
        } else if controllers.split(',').any(|c| c == "memory") {
            limits.extend(up("/sys/fs/cgroup/memory", &path, "memory.limit_in_bytes"));
        }
    }
    if limits.is_empty() {
        limits.extend(read(Path::new("/sys/fs/cgroup/memory.max")));
        limits.extend(read(Path::new(
            "/sys/fs/cgroup/memory/memory.limit_in_bytes",
        )));
    }
    limits
        .iter()
        .filter_map(|text| {
            let text = text.trim();
            (text != "max")
                .then(|| text.parse::<u64>().ok())
                .flatten()
                .filter(|limit| *limit < UNLIMITED)
        })
        .min()
        .filter(|limit| total.is_none_or(|total| *limit < total))
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn cgroups() -> Vec<(String, String)> {
    read(Path::new("/proc/self/cgroup"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ':');
            let (_, controllers, path) = (parts.next()?, parts.next()?, parts.next()?);
            Some((controllers.to_owned(), path.to_owned()))
        })
        .collect()
}

fn up(root: &str, cgroup: &str, file: &str) -> Vec<String> {
    let mut dir = PathBuf::from(root).join(cgroup.trim_start_matches('/'));
    let mut found = Vec::new();
    loop {
        if let Some(text) = read(&dir.join(file)) {
            found.push(text);
        }
        if dir == Path::new(root) || !dir.pop() {
            break;
        }
    }
    found
}

fn used(file: &Path) -> u64 {
    read(file)
        .and_then(|used| used.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn usage_file() -> Option<PathBuf> {
    cgroups().into_iter().find_map(|(controllers, path)| {
        let path = path.trim_start_matches('/');
        let file = if controllers.is_empty() {
            Path::new("/sys/fs/cgroup")
                .join(path)
                .join("memory.current")
        } else if controllers.split(',').any(|c| c == "memory") {
            Path::new("/sys/fs/cgroup/memory")
                .join(path)
                .join("memory.usage_in_bytes")
        } else {
            return None;
        };
        file.exists().then_some(file)
    })
}

async fn watch(pid: Option<u32>, limit: u64, together: Option<Together<'_>>) -> Stopped {
    let Some(pid) = pid else {
        return std::future::pending().await;
    };
    let mut adjusted = BTreeSet::new();
    let own = std::env::current_exe().ok();
    let every = if together.is_some() {
        SHELL_EVERY
    } else {
        MEMORY_EVERY
    };
    loop {
        let tree = processes(pid);
        let resident = holding(&tree, own.as_deref(), &mut adjusted);
        let stop = if resident > limit {
            Some((Stop::Call, limit))
        } else {
            together
                .as_ref()
                .filter(|together| together.over(resident))
                .map(|together| (Stop::Together, together.most))
        };
        if let Some((stop, most)) = stop {
            kill(&tree);
            return Stopped {
                stop,
                resident,
                processes: tree.len(),
                most,
            };
        }
        nanocode_platform::sleep(every).await;
    }
}

fn holding(tree: &[u32], own: Option<&Path>, adjusted: &mut BTreeSet<u32>) -> u64 {
    for process in tree {
        if adjusted.insert(*process) {
            std::fs::write(format!("/proc/{process}/oom_score_adj"), "1000").ok();
        }
    }
    tree.iter()
        .filter(|process| {
            !own.is_some_and(|own| {
                std::fs::read_link(format!("/proc/{process}/exe")).is_ok_and(|exe| exe == own)
            })
        })
        .map(|process| resident(*process))
        .sum()
}

fn kill(tree: &[u32]) {
    for process in tree {
        if let Some(process) = i32::try_from(*process)
            .ok()
            .filter(|process| *process != 0)
        {
            nanocode_platform::unix::signal_process(process, libc::SIGKILL).ok();
        }
    }
}

fn processes(pid: u32) -> Vec<u32> {
    let mut found = vec![pid];
    let mut next = 0;
    while let Some(&process) = found.get(next) {
        next += 1;
        let Ok(threads) = std::fs::read_dir(format!("/proc/{process}/task")) else {
            continue;
        };
        for thread in threads.flatten() {
            let children =
                std::fs::read_to_string(thread.path().join("children")).unwrap_or_default();
            found.extend(
                children
                    .split_whitespace()
                    .filter_map(|child| child.parse::<u32>().ok()),
            );
        }
    }
    found
}

fn resident(pid: u32) -> u64 {
    read(Path::new(&format!("/proc/{pid}/status")))
        .and_then(|status| {
            let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        })
        .map_or(0, |kib| kib << 10)
}
