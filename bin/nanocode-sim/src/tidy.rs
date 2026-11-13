use std::path::{Path, PathBuf};

const SIMULATED: &[&str] = &[
    "crates/backends/claude/src",
    "crates/core/agent/src",
    "crates/core/durability/src",
    "crates/core/sync/src",
    "crates/core/turnlog/src",
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
const UNBIASED: &str = "draws its first branch from tokio's thread RNG; start it with biased;";

const BANNED: &[(&str, &str)] = &[
    ("SystemTime::now", BYPASSES_HOST),
    ("Instant::now", BYPASSES_HOST),
    ("thread::spawn", BYPASSES_HOST),
    ("thread::Builder", BYPASSES_HOST),
    ("getrandom", BYPASSES_HOST),
    ("tokio::time", BYPASSES_HOST),
    ("tokio::spawn", BYPASSES_HOST),
    ("Handle::try_current", BYPASSES_HOST),
    ("FuturesUnordered", WAKE_ORDER),
    ("FuturesOrdered", JOIN_ORDER),
    ("join_all", JOIN_ORDER),
];

const SELECT: &str = "select!";
const BIASED: &str = "biased;";

const TOKIO_GROUP: &str = "use tokio::{";

struct Allowed {
    path: &'static str,
    call: &'static str,
    uses: usize,
    why: &'static str,
}

const ALLOWED: &[Allowed] = &[
    Allowed {
        path: "crates/backends/claude/src/claude_bash.rs",
        call: "Instant::now",
        uses: 3,
        why: "Claude Code's Bash tool watches a real shell process for silence on Tokio's clock; the simulator registers no Bash tool",
    },
    Allowed {
        path: "crates/backends/claude/src/claude_bash.rs",
        call: "select!",
        uses: 1,
        why: "Claude Code's Bash tool races a real process's exit against its output; the simulator registers no Bash tool",
    },
    Allowed {
        path: "crates/backends/claude/src/claude_bash.rs",
        call: "tokio::spawn",
        uses: 5,
        why: "Claude Code's Bash tool reads and reaps real shell processes from Tokio tasks; the simulator registers no Bash tool",
    },
    Allowed {
        path: "crates/backends/claude/src/claude_bash.rs",
        call: "tokio::time",
        uses: 6,
        why: "Claude Code's Bash tool times real shell processes on Tokio's clock; the simulator registers no Bash tool",
    },
    Allowed {
        path: "crates/backends/claude/src/claude_tools.rs",
        call: "Handle::try_current",
        uses: 1,
        why: "a dropped tool runtime guard cancels its real tool runtime on the caller's Tokio runtime; the simulator builds no tool runtime",
    },
    Allowed {
        path: "crates/backends/claude/src/host_tools.rs",
        call: "tokio::spawn",
        uses: 1,
        why: "Claude Code's host tools run real files and processes in the background on Tokio; the simulator registers no host tools",
    },
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
        .map_or(UNBIASED, |(_, problem)| problem)
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
            let mut opened = None;
            for (number, line) in text.lines().enumerate() {
                let mut calls: Vec<(&'static str, usize)> = found(line, &mut group)
                    .into_iter()
                    .map(|call| (call, number))
                    .collect();
                if let Some(select) = opened.take()
                    && !line.trim_start().starts_with(BIASED)
                {
                    calls.push((SELECT, select));
                }
                if line.contains(SELECT) && line.trim_end().ends_with('{') {
                    opened = Some(number);
                }
                for (call, at) in calls {
                    let allowed = ALLOWED
                        .iter()
                        .position(|allowed| allowed.path == relative && allowed.call == call);
                    match allowed {
                        Some(index) => used[index] += 1,
                        None => problems.push(format!(
                            "{relative}:{}: {call} {}",
                            at + 1,
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
