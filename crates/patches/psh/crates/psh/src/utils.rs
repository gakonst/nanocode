//! `psh --utils [--json]` (D1341): psh's in-process utilities from the registry, and
//! what a command of each name runs here. The answer comes from a shell configured as
//! the standalone shell is (`shell_config`: PATH, `PSH_UTILS_FORCE`, Windows), asked
//! with `type -a`, so it is the engine's own lookup and cannot drift from what runs.

use std::cell::RefCell;
use std::io::{self, Write};
use std::process::ExitCode;
use std::rc::Rc;
use std::task::{Context, Poll};

use psh_engine::std_host::{drive, engine_thread};
use psh_engine::util::Mode;
use psh_engine::{HostFile, Script, Shell};

use crate::json::Json;

/// What a command of a utility's name runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Runs {
    /// psh's utility.
    Psh,
    /// The program `PATH` finds.
    Program,
    /// A shell builtin of that name, which comes before both.
    Builtin,
    /// Nothing (cannot happen for a utility that is on, but `type` is the judge).
    Nothing,
}

impl Runs {
    fn name(self) -> &'static str {
        match self {
            Runs::Psh => "psh",
            Runs::Program => "program",
            Runs::Builtin => "builtin",
            Runs::Nothing => "nothing",
        }
    }
}

/// One name of a utility.
struct Row {
    name: &'static str,
    /// The entry's first name (`timeout` for `gtimeout`).
    utility: &'static str,
    mode: Mode,
    destructive: bool,
    runs: Runs,
    /// The program `PATH` has for the name, whether or not it runs.
    path: Option<Vec<u8>>,
}

fn mode_letter(m: Mode) -> &'static str {
    match m {
        Mode::Always => "S",
        Mode::PathFallback => "F",
        Mode::WindowsShadow => "W",
        Mode::SandboxOnly => "X",
    }
}

/// The shell's stdout, kept in memory.
struct Capture(RefCell<Vec<u8>>);

impl HostFile for Capture {
    fn poll_read(&self, _: &mut Context<'_>, _: &mut [u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(0))
    }

    fn poll_write(&self, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.0.borrow_mut().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }
}

/// Every name of the registry, in its order.
fn names() -> impl Iterator<Item = (&'static str, &'static psh_engine::util::UtilEntry)> {
    psh_utils::REGISTRY
        .iter()
        .flat_map(|e| e.names.iter().map(move |n| (*n, e)))
}

/// `type -a` for every name, in a shell configured as `psh -c` would be.
fn ask_shell() -> Option<Vec<u8>> {
    // The names are plain words (checked by a test), so they need no quoting.
    let list: Vec<&str> = names().map(|(n, _)| n).collect();
    let script = format!(
        "for n in {}; do type -a -- \"$n\"; done 2>/dev/null",
        list.join(" ")
    );
    let thread = engine_thread(move || {
        let mut cfg = crate::shell_config();
        let out = Rc::new(Capture(RefCell::new(Vec::new())));
        cfg.stdout = Some(out.clone());
        cfg.stdin = None;
        let mut shell = Shell::new(cfg);
        shell.start(Script::command(&script));
        drive(&mut shell);
        drop(shell);
        out.0.take()
    });
    thread.ok()?.join().ok()
}

/// The rows from `type -a`'s lines (`NAME is a psh utility`, `NAME is /usr/bin/NAME`, `NAME is a shell builtin`).
fn rows(out: &[u8]) -> Vec<Row> {
    let mut rows: Vec<Row> = names()
        .map(|(name, e)| Row {
            name,
            utility: e.names[0],
            mode: e.mode,
            destructive: e.destructive,
            runs: Runs::Nothing,
            path: None,
        })
        .collect();
    for line in out.split(|&b| b == b'\n') {
        let Some(at) = line.windows(4).position(|w| w == b" is ") else {
            continue;
        };
        let (name, what) = (&line[..at], &line[at + 4..]);
        let Some(row) = rows.iter_mut().find(|r| r.name.as_bytes() == name) else {
            continue;
        };
        let kind = match what {
            b"a psh utility" => Runs::Psh,
            b"a shell builtin" => Runs::Builtin,
            path => {
                if row.path.is_none() {
                    row.path = Some(path.to_vec());
                }
                Runs::Program
            }
        };
        // `type -a` lists what runs first.
        if row.runs == Runs::Nothing {
            row.runs = kind;
        }
    }
    rows
}

/// Prints the list; status 2 when the shell could not be asked or stdout fails.
pub fn list(json: bool) -> ExitCode {
    let Some(out) = ask_shell() else {
        eprintln!("psh: --utils: cannot start a shell");
        return ExitCode::from(2);
    };
    let rows = rows(&out);
    let text = if json {
        let list = rows
            .iter()
            .map(|r| {
                Json::obj()
                    .set("name", Json::Str(r.name.into()))
                    .set("utility", Json::Str(r.utility.into()))
                    .set("mode", Json::Str(mode_letter(r.mode).into()))
                    .set("destructive", r.destructive)
                    .set("runs", Json::Str(r.runs.name().into()))
                    .set(
                        "path",
                        match &r.path {
                            Some(p) => Json::Str(String::from_utf8_lossy(p).into_owned()),
                            None => Json::Null,
                        },
                    )
            })
            .collect();
        Json::obj().set("utilities", Json::Arr(list)).to_pretty() + "\n"
    } else {
        table(&rows)
    };
    let mut stdout = io::stdout().lock();
    if stdout
        .write_all(text.as_bytes())
        .and_then(|_| stdout.flush())
        .is_err()
    {
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

fn table(rows: &[Row]) -> String {
    let width = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let mut s = format!("{:width$}  MODE  DESTRUCTIVE  RUNS     PATH\n", "NAME");
    for r in rows {
        let path = r
            .path
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or_default();
        let line = format!(
            "{:width$}  {:4}  {:11}  {:7}  {}",
            r.name,
            mode_letter(r.mode),
            if r.destructive { "yes" } else { "no" },
            r.runs.name(),
            path
        );
        s.push_str(line.trim_end());
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_need_no_quoting() {
        for (n, _) in names() {
            assert!(
                !n.is_empty()
                    && n.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "{n}"
            );
        }
    }

    #[test]
    fn rows_from_type() {
        let out = b"cat is a psh utility\ncat is /bin/cat\ncat is /usr/bin/cat\n\
                    awk is /usr/bin/awk\nrm is a psh utility\n";
        let rows = rows(out);
        let row = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(row("cat").runs, Runs::Psh);
        assert_eq!(row("cat").path.as_deref(), Some(&b"/bin/cat"[..]));
        assert_eq!(row("awk").runs, Runs::Program);
        assert_eq!(row("gawk").utility, "awk");
        assert_eq!(row("gawk").runs, Runs::Nothing);
        assert!(row("rm").destructive && row("rm").path.is_none());
    }
}
