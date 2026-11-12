use std::path::{Path, PathBuf};

const SIMULATED: &[&str] = &[
    "crates/core/durability/src",
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

struct Allowed {
    path: &'static str,
    call: &'static str,
    why: &'static str,
}

const ALLOWED: &[Allowed] = &[
    Allowed {
        path: "crates/runtime/durability/src/shared_store.rs",
        call: "thread::Builder",
        why: "without a host, SharedStore drivers run on one current_thread runtime on their own thread; spawn_driver gives them to an installed host first",
    },
    Allowed {
        path: "crates/runtime/durability/src/memory.rs",
        call: "Handle::try_current",
        why: "without a host, the in-memory store's driver runs on the caller's Tokio runtime; spawn_driver gives it to an installed host first",
    },
    Allowed {
        path: "crates/runtime/durability/src/session.rs",
        call: "Handle::try_current",
        why: "without a host, session drivers and claim acknowledgements run on the caller's Tokio runtime; both give them to an installed host first",
    },
    Allowed {
        path: "crates/runtime/durability/src/child_journal.rs",
        call: "Handle::try_current",
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

pub fn check(root: &Path) -> std::io::Result<Vec<String>> {
    let mut problems = Vec::new();
    let mut used = vec![false; ALLOWED.len()];
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
            for (number, line) in text.lines().enumerate() {
                for call in BANNED.iter().filter(|call| line.contains(**call)) {
                    let allowed = ALLOWED
                        .iter()
                        .position(|allowed| allowed.path == relative && allowed.call == *call);
                    match allowed {
                        Some(at) => used[at] = true,
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
        if !used {
            problems.push(format!(
                "{}: the allowance for {} is no longer used",
                allowed.path, allowed.call
            ));
        }
    }
    Ok(problems)
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
