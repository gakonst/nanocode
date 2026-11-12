use std::path::{Path, PathBuf};

const SIMULATED: &[&str] = &[
    "crates/core/agent/src",
    "crates/core/durability/src",
    "crates/core/sync/src",
    "crates/integrations/store-jsonl/src",
    "crates/runtime/agent/src",
    "crates/runtime/durability/src",
    "crates/runtime/subagents/src",
];

const BANNED: &[&str] = &[
    "SystemTime::now",
    "Instant::now",
    "thread::spawn",
    "thread::Builder",
    "getrandom",
    "tokio::time",
    "tokio::spawn",
    "Handle::try_current",
];

const TOKIO_GROUP: &str = "use tokio::{";

struct Allowed {
    path: &'static str,
    call: &'static str,
    uses: usize,
    why: &'static str,
}

const ALLOWED: &[Allowed] = &[
    Allowed {
        path: "crates/core/agent/src/agent/handle.rs",
        call: "Handle::try_current",
        uses: 1,
        why: "without a host, a claimed turn's result is forwarded by a task on the caller's Tokio runtime, or in place outside one; an installed host takes it first",
    },
    Allowed {
        path: "crates/core/agent/src/agent/owned.rs",
        call: "Handle::try_current",
        uses: 1,
        why: "without a host, detached event publishing runs on the caller's Tokio runtime; spawn_detached gives it to an installed host first",
    },
    Allowed {
        path: "crates/core/sync/src/lanes.rs",
        call: "tokio::spawn",
        uses: 1,
        why: "Lanes batch a host's commits over its own connections (pyramid-core); no simulated crate builds one",
    },
    Allowed {
        path: "crates/core/sync/src/lanes.rs",
        call: "tokio::time",
        uses: 1,
        why: "Lanes batch a host's commits over its own connections (pyramid-core); no simulated crate builds one",
    },
    Allowed {
        path: "crates/core/sync/src/lease.rs",
        call: "Instant::now",
        uses: 3,
        why: "a lease's watchdog measures renewals on its own thread so that it expires the lease even when the Tokio runtime stalls; leases guard a host's external locks and no simulated crate takes one",
    },
    Allowed {
        path: "crates/core/sync/src/lease.rs",
        call: "thread::spawn",
        uses: 1,
        why: "a lease's watchdog runs on its own thread so that it expires the lease even when the Tokio runtime stalls",
    },
    Allowed {
        path: "crates/core/sync/src/lease.rs",
        call: "tokio::spawn",
        uses: 1,
        why: "a lease's renewals run as a Tokio task beside its watchdog thread; no simulated crate takes a lease",
    },
    Allowed {
        path: "crates/core/sync/src/lease.rs",
        call: "tokio::time",
        uses: 1,
        why: "a lease's renewals sleep on Tokio's clock beside its watchdog thread; no simulated crate takes a lease",
    },
    Allowed {
        path: "crates/core/sync/src/lib.rs",
        call: "Handle::try_current",
        uses: 2,
        why: "without a host, an Actor's server and a Mailbox hand-off run on the caller's Tokio runtime, and a hand-off outside one delivers in place; both go to an installed host first",
    },
    Allowed {
        path: "crates/core/sync/src/lib.rs",
        call: "tokio::spawn",
        uses: 1,
        why: "without a host, a Recent read runs as a Tokio task whose panic resumes in its reader; detached gives it to an installed host first",
    },
    Allowed {
        path: "crates/core/sync/src/session.rs",
        call: "Instant::now",
        uses: 2,
        why: "session locks wait on a host's SQL connections against Tokio deadlines; no simulated crate takes one",
    },
    Allowed {
        path: "crates/core/sync/src/session.rs",
        call: "tokio::spawn",
        uses: 1,
        why: "session locks keep their SQL connections alive from Tokio tasks; no simulated crate takes one",
    },
    Allowed {
        path: "crates/core/sync/src/session.rs",
        call: "tokio::time",
        uses: 3,
        why: "session locks wait on a host's SQL connections against Tokio deadlines; no simulated crate takes one",
    },
    Allowed {
        path: "crates/core/sync/src/splice.rs",
        call: "Instant::now",
        uses: 2,
        why: "splice joins two TCP streams for a host's network egress until they idle; the simulator runs no sockets",
    },
    Allowed {
        path: "crates/core/sync/src/splice.rs",
        call: "tokio::time",
        uses: 1,
        why: "splice joins two TCP streams for a host's network egress until they idle; the simulator runs no sockets",
    },
    Allowed {
        path: "crates/core/sync/src/waits.rs",
        call: "Instant::now",
        uses: 2,
        why: "slow-wait diagnostics time how long a holder waited for slow_waits reports, never a decision or persisted bytes, on whatever thread holds it",
    },
    Allowed {
        path: "crates/integrations/store-jsonl/src/lib.rs",
        call: "Instant::now",
        uses: 5,
        why: "span durations (append, encode, write, queue wait) timed with the real clock, mostly on the writer's own thread where no host runs; they reach tracing fields only",
    },
    Allowed {
        path: "crates/integrations/store-jsonl/src/lib.rs",
        call: "thread::Builder",
        uses: 2,
        why: "blocking file I/O runs on the store's owning writer and syncer threads behind channels, as AGENTS.md requires",
    },
    Allowed {
        path: "crates/runtime/agent/src/runtime.rs",
        call: "Handle::try_current",
        uses: 1,
        why: "build_async opens the default store and assembles the runtime on its own thread, off the caller's Tokio worker; a host builds with build",
    },
    Allowed {
        path: "crates/runtime/agent/src/runtime.rs",
        call: "thread::Builder",
        uses: 1,
        why: "build_async opens the default store and assembles the runtime on its own thread, off the caller's Tokio worker; a host builds with build",
    },
    Allowed {
        path: "crates/runtime/durability/src/shared_store.rs",
        call: "thread::Builder",
        uses: 1,
        why: "without a host, SharedStore drivers run on one current_thread runtime on their own thread; spawn_driver gives them to an installed host first",
    },
    Allowed {
        path: "crates/runtime/durability/src/memory.rs",
        call: "Handle::try_current",
        uses: 1,
        why: "without a host, the in-memory store's driver runs on the caller's Tokio runtime; spawn_driver gives it to an installed host first",
    },
    Allowed {
        path: "crates/runtime/durability/src/session.rs",
        call: "Handle::try_current",
        uses: 2,
        why: "without a host, session drivers and claim acknowledgements run on the caller's Tokio runtime; both give them to an installed host first",
    },
    Allowed {
        path: "crates/runtime/durability/src/child_journal.rs",
        call: "Handle::try_current",
        uses: 1,
        why: "without a host, the child journal writer runs on the caller's Tokio runtime; spawn_writer gives it to an installed host first",
    },
];

fn sources(directory: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    Ok(())
}

fn grouped(member: &str) -> Option<&'static str> {
    let member = member.trim();
    let name = member.split_whitespace().next().unwrap_or_default();
    if name == "spawn" {
        return Some("tokio::spawn");
    }
    if name == "time" || name.starts_with("time::") {
        return Some("tokio::time");
    }
    None
}

#[derive(Default)]
struct Group {
    depth: usize,
}

impl Group {
    fn calls(&mut self, line: &str) -> Vec<&'static str> {
        let inside = if self.depth > 0 {
            line
        } else if let Some(at) = line.find(TOKIO_GROUP) {
            self.depth = 1;
            &line[at + TOKIO_GROUP.len()..]
        } else {
            return Vec::new();
        };
        let mut calls = Vec::new();
        let mut member = String::new();
        let mut at_depth = self.depth;
        for character in inside.chars() {
            match character {
                '{' => {
                    if at_depth == 1 {
                        calls.extend(grouped(&member));
                    }
                    member.clear();
                    self.depth += 1;
                }
                '}' => {
                    if self.depth == 1 {
                        calls.extend(grouped(&member));
                    }
                    member.clear();
                    self.depth -= 1;
                    if self.depth == 0 {
                        break;
                    }
                }
                ',' => {
                    if self.depth == 1 {
                        calls.extend(grouped(&member));
                    }
                    member.clear();
                }
                _ => member.push(character),
            }
            at_depth = self.depth;
        }
        if self.depth == 1 {
            calls.extend(grouped(&member));
        }
        calls
    }
}

fn found(line: &str, group: &mut Group) -> Vec<&'static str> {
    let mut calls: Vec<&'static str> = BANNED
        .iter()
        .copied()
        .filter(|call| line.contains(*call))
        .collect();
    for call in group.calls(line) {
        if !calls.contains(&call) {
            calls.push(call);
        }
    }
    calls
}

pub fn check(root: &Path) -> std::io::Result<Vec<String>> {
    let mut problems = Vec::new();
    let mut used = vec![0_usize; ALLOWED.len()];
    for crate_sources in SIMULATED {
        let mut files = Vec::new();
        sources(&root.join(crate_sources), &mut files)?;
        for file in files {
            let relative = file
                .strip_prefix(root)
                .expect("a scanned file lies under the workspace root")
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read_to_string(&file)?;
            let mut group = Group::default();
            for (number, line) in text.lines().enumerate() {
                for call in found(line, &mut group) {
                    let allowed = ALLOWED
                        .iter()
                        .position(|allowed| allowed.path == relative && allowed.call == call);
                    match allowed {
                        Some(at) => used[at] += 1,
                        None => problems.push(format!(
                            "{relative}:{}: {call} bypasses nanocode-platform's host",
                            number + 1
                        )),
                    }
                }
            }
        }
    }
    for (allowed, used) in ALLOWED.iter().zip(used) {
        assert!(!allowed.why.is_empty(), "every allowance says why");
        assert!(allowed.uses > 0, "every allowance counts what it allows");
        if used != allowed.uses {
            problems.push(format!(
                "{}: {used} uses of {} where the allowance counts {}",
                allowed.path, allowed.call, allowed.uses
            ));
        }
    }
    Ok(problems)
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
