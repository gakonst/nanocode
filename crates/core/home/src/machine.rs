//! What the machine, or the cgroup a program runs in, has.

use std::fs;

/// Memory used and in all, in bytes. Under a cgroup v2 limit below the
/// machine's memory, the cgroup's use without its inactive file cache (which
/// the kernel reclaims first) and its limit; else the machine's memory less what
/// it has available. `None` without `/proc/meminfo` (not Linux).
#[must_use]
pub fn memory() -> Option<(u64, u64)> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let cgroup = |name: &str| fs::read_to_string(format!("/sys/fs/cgroup/{name}")).ok();
    measure(
        &meminfo,
        cgroup("memory.max").as_deref(),
        cgroup("memory.current").as_deref(),
        cgroup("memory.stat").as_deref(),
    )
}

/// [`memory`] from the texts of `/proc/meminfo` and the cgroup's `memory.max`,
/// `memory.current` and `memory.stat`.
#[must_use]
pub fn measure(
    meminfo: &str,
    max: Option<&str>,
    current: Option<&str>,
    stat: Option<&str>,
) -> Option<(u64, u64)> {
    let kib = |key: &str| {
        meminfo.lines().find_map(|line| {
            let rest = line.strip_prefix(key)?.strip_prefix(':')?;
            rest.split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|n| n * 1024)
        })
    };
    let total = kib("MemTotal").filter(|total| *total > 0)?;
    let limit = max.and_then(|max| max.trim().parse::<u64>().ok());
    if let (Some(limit), Some(current)) =
        (limit.filter(|limit| *limit > 0 && *limit < total), current)
    {
        let current = current.trim().parse::<u64>().ok()?;
        let inactive = stat
            .and_then(|stat| {
                stat.lines().find_map(|line| {
                    line.strip_prefix("inactive_file ")?
                        .trim()
                        .parse::<u64>()
                        .ok()
                })
            })
            .unwrap_or(0);
        return Some((current.saturating_sub(inactive).min(limit), limit));
    }
    Some((total.saturating_sub(kib("MemAvailable")?), total))
}
