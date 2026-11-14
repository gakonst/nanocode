use std::path::{Path, PathBuf};

const SIMULATED: &[&str] = &[
    "crates/backends/claude/src",
    "crates/core/agent/src",
    "crates/core/durability/src",
    "crates/core/sync/src",
    "crates/core/turnlog/src",
    "crates/core/websocket/src",
    "crates/integrations/store-jsonl/src",
    "crates/integrations/store-sql/src",
    "crates/runtime/agent/src",
    "crates/runtime/code-mode-engine/src",
    "crates/runtime/code-mode/src/code_mode",
    "crates/runtime/durability/src",
    "crates/runtime/subagents/src",
    "crates/runtime/tool-host/src",
];

const BYPASSES_HOST: &str = "bypasses nanocode-platform's host";
const WAKE_ORDER: &str = "yields its futures in the order they were woken, which tokio's watch and HashMap iteration draw at random; use nanocode_sync::InOrder";
const JOIN_ORDER: &str = "polls in the order its futures were woken (join_all does past 30 futures, through FuturesOrdered); use nanocode_sync::join_in_order or try_join_in_order";

const BANNED: &[(&str, &str)] = &[
    ("SystemTime::now", BYPASSES_HOST),
    ("Instant::now", BYPASSES_HOST),
    ("thread::spawn", BYPASSES_HOST),
    ("thread::Builder", BYPASSES_HOST),
    ("getrandom", BYPASSES_HOST),
    ("nanocode_executor::sleep", BYPASSES_HOST),
    ("nanocode_executor::timeout", BYPASSES_HOST),
    ("nanocode_executor::spawn", BYPASSES_HOST),
    ("nanocode_executor::Builder", BYPASSES_HOST),
    ("nanocode_executor::Handle", BYPASSES_HOST),
    ("FuturesUnordered", WAKE_ORDER),
    ("FuturesOrdered", JOIN_ORDER),
    ("join_all", JOIN_ORDER),
];

const EXECUTOR_GROUP: &str = "use nanocode_executor::{";

struct Allowed {
    path: &'static str,
    call: &'static str,
    uses: usize,
    why: &'static str,
}

const ALLOWED: &[Allowed] = &[
    Allowed {
        path: "crates/core/sync/src/future.rs",
        call: "FuturesOrdered",
        uses: 3,
        why: "nanocode-sync's own join_all, which keeps futures-util's order: up to 30 futures polled together, past that through FuturesOrdered; simulated crates still use join_in_order",
    },
    Allowed {
        path: "crates/core/sync/src/future.rs",
        call: "join_all",
        uses: 2,
        why: "nanocode-sync's own join_all, which keeps futures-util's order: up to 30 futures polled together, past that through FuturesOrdered; simulated crates still use join_in_order",
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
        path: "crates/core/sync/src/stream.rs",
        call: "FuturesOrdered",
        uses: 3,
        why: "nanocode-sync's own FuturesOrdered, which keeps futures-util's order: outputs in push order, futures polled in wake order; simulated crates still join through join_in_order",
    },
    Allowed {
        path: "crates/core/sync/src/stream.rs",
        call: "FuturesUnordered",
        uses: 1,
        why: "nanocode-sync's own FuturesUnordered, which keeps futures-util's order: futures polled in the order they were woken; simulated crates still use InOrder",
    },
    Allowed {
        path: "crates/core/sync/src/unordered.rs",
        call: "FuturesOrdered",
        uses: 8,
        why: "nanocode-sync's own FuturesOrdered, which keeps futures-util's order: outputs in push order, futures polled in wake order; simulated crates still join through join_in_order",
    },
    Allowed {
        path: "crates/core/sync/src/unordered.rs",
        call: "FuturesUnordered",
        uses: 7,
        why: "nanocode-sync's own FuturesUnordered, which keeps futures-util's order: futures polled in the order they were woken; simulated crates still use InOrder",
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
        call: "thread::Builder",
        uses: 1,
        why: "build_async opens the default store and assembles the runtime on its own thread, off the caller's Tokio worker; a host builds with build",
    },
    Allowed {
        path: "crates/runtime/durability/src/shared_store.rs",
        call: "nanocode_executor::Handle",
        uses: 1,
        why: "without a host, SharedStore drivers run on one current_thread runtime on their own thread; spawn_driver gives them to an installed host first",
    },
    Allowed {
        path: "crates/runtime/durability/src/shared_store.rs",
        call: "nanocode_executor::Builder",
        uses: 1,
        why: "without a host, SharedStore drivers run on one current_thread runtime on their own thread; spawn_driver gives them to an installed host first",
    },
    Allowed {
        path: "crates/runtime/durability/src/shared_store.rs",
        call: "thread::Builder",
        uses: 1,
        why: "without a host, SharedStore drivers run on one current_thread runtime on their own thread; spawn_driver gives them to an installed host first",
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
    if name.starts_with("spawn") {
        return Some("nanocode_executor::spawn");
    }
    if name.starts_with("sleep") {
        return Some("nanocode_executor::sleep");
    }
    if name.starts_with("timeout") {
        return Some("nanocode_executor::timeout");
    }
    if name == "Builder" {
        return Some("nanocode_executor::Builder");
    }
    if name == "Handle" {
        return Some("nanocode_executor::Handle");
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
        } else if let Some(at) = line.find(EXECUTOR_GROUP) {
            self.depth = 1;
            &line[at + EXECUTOR_GROUP.len()..]
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
        .map(|(call, _)| *call)
        .filter(|call| line.contains(*call))
        .collect();
    for call in group.calls(line) {
        if !calls.contains(&call) {
            calls.push(call);
        }
    }
    calls
}

fn problem(call: &str) -> &'static str {
    BANNED
        .iter()
        .find(|(banned, _)| *banned == call)
        .map(|(_, problem)| *problem)
        .expect("every reported call is banned")
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
                        Some(index) => used[index] += 1,
                        None => problems.push(format!(
                            "{relative}:{}: {call} {}",
                            number + 1,
                            problem(call)
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
