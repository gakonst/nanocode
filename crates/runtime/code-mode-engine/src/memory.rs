#[cfg(target_os = "linux")]
use std::{fs, path::Path};

const ARENA_COPIES: u64 = 2;

pub(crate) fn machine_cell_memory() -> usize {
    available()
        .map(|bytes| bytes / ARENA_COPIES)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .unwrap_or(usize::MAX)
}

#[cfg(target_os = "linux")]
const KIB: u64 = 1024;

#[cfg(target_os = "linux")]
fn available() -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let machine = meminfo
        .lines()
        .find_map(|line| {
            line.strip_prefix("MemAvailable:")?
                .trim()
                .strip_suffix("kB")?
                .trim()
                .parse::<u64>()
                .ok()
        })?
        .checked_mul(KIB)?;
    let cgroups = fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let Some(path) = cgroups.lines().find_map(|line| line.strip_prefix("0::")) else {
        return Some(machine);
    };
    let root = Path::new("/sys/fs/cgroup");
    let mut dir = root.join(path.trim_start_matches('/'));
    let mut room = machine;
    loop {
        if let Some(free) = cgroup_room(&dir) {
            room = room.min(free);
        }
        if dir == root || !dir.pop() {
            return Some(room);
        }
    }
}

#[cfg(target_os = "linux")]
fn cgroup_room(dir: &Path) -> Option<u64> {
    let read = |name: &str| fs::read_to_string(dir.join(name)).ok();
    let max = read("memory.max")?.trim().parse::<u64>().ok()?;
    let current = read("memory.current")?.trim().parse::<u64>().ok()?;
    let inactive = read("memory.stat")
        .and_then(|stat| {
            stat.lines().find_map(|line| {
                line.strip_prefix("inactive_file ")?
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
        })
        .unwrap_or(0);
    Some(max.saturating_sub(current.saturating_sub(inactive)))
}

#[cfg(not(target_os = "linux"))]
const fn available() -> Option<u64> {
    None
}
