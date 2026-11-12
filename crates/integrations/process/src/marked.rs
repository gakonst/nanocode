use std::{collections::HashSet, time::Duration};

pub fn terminate_marked(variable: &str, value: &str, grace: Duration) -> usize {
    let needle = format!("{variable}={value}");
    let (me, group) = (
        std::process::id().cast_signed(),
        nanocode_platform::unix::process_group(),
    );
    let escaped = || -> Vec<i32> {
        marked(&needle)
            .into_iter()
            .filter(|&(pid, of)| pid != me && of != group)
            .map(|(pid, _)| pid)
            .collect()
    };
    let mut ended: HashSet<i32> = escaped().into_iter().collect();
    if ended.is_empty() {
        return 0;
    }
    for &pid in &ended {
        let _ = nanocode_platform::unix::signal_process(pid, libc::SIGTERM);
    }
    let until = std::time::Instant::now() + grace;
    while std::time::Instant::now() < until && !escaped().is_empty() {
        std::thread::sleep(Duration::from_millis(50));
    }
    for _ in 0..5 {
        let found = escaped();
        for &pid in &found {
            let _ = nanocode_platform::unix::signal_process(pid, libc::SIGKILL);
        }
        if found.is_empty() {
            break;
        }
        ended.extend(found);
    }
    ended.len()
}

#[cfg(target_os = "linux")]
fn marked(needle: &str) -> Vec<(i32, i32)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let marked = |pid: i32| -> Option<(i32, i32)> {
        let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        environ
            .split(|&byte| byte == 0)
            .any(|variable| variable == needle.as_bytes())
            .then_some(())?;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        Some((
            pid,
            stat.rsplit_once(')')?
                .1
                .split_whitespace()
                .nth(2)?
                .parse()
                .ok()?,
        ))
    };
    entries
        .flatten()
        .filter_map(|entry| marked(entry.file_name().to_str()?.parse().ok()?))
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn marked(needle: &str) -> Vec<(i32, i32)> {
    let ps = std::process::Command::new("/bin/ps")
        .args(["-axwwE", "-o", "pid=,pgid=,command="])
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(output) = ps else {
        return Vec::new();
    };
    let parse = |line: &str| {
        let mut words = line.split_whitespace();
        let (pid, group) = (words.next()?.parse().ok()?, words.next()?.parse().ok()?);
        words.any(|word| word == needle).then_some((pid, group))
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse)
        .collect()
}
