//! The shell exports: create a shell over the JS host, start a script, step it, cancel
//! and free it. The JS side drives `psh_shell_step` from its event loop: on `Blocked`
//! it waits until the module asks it to wake the shell (`K_WAKE`), which happens when
//! one of its answers unblocks a task.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Wake, Waker};

use psh_engine::{HostFile, Script, Shell, ShellConfig, Step};

use crate::host::{JsFile, JsHost};
use crate::{export, sys, take_input};

/// Step results other than `Done(status)` (a status is 0..=255).
pub const STEP_BLOCKED: i32 = -1;
pub const STEP_YIELD: i32 = -2;
pub const STEP_IDLE: i32 = -3;
/// No such shell.
pub const STEP_BAD: i32 = -4;

thread_local! {
    static SHELLS: RefCell<Vec<Option<Entry>>> = const { RefCell::new(Vec::new()) };
}

struct Entry {
    shell: Shell,
    /// A script was started and has not reported `Done` yet.
    running: bool,
}

struct WakeShell(u32);

impl Wake for WakeShell {
    fn wake(self: Arc<Self>) {
        sys::call(sys::K_WAKE, 0, self.0, &[]);
    }
}

/// Reads the length-prefixed fields of a payload.
struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Option<u32> {
        let (head, rest) = self.data.split_at_checked(4)?;
        self.data = rest;
        Some(u32::from_le_bytes(head.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        let (head, rest) = self.data.split_at_checked(n)?;
        self.data = rest;
        Some(head)
    }
}

/// The stack the module links with (build.rs) and the nesting limit it allows: each
/// level may take up to [`ShellConfig::STACK_PER_LEVEL`]; a quarter of the stack stays
/// free for the rest of the call chain.
const STACK: usize = 16 * 1024 * 1024;
pub const MAX_DEPTH: u32 = (STACK * 3 / 4 / ShellConfig::STACK_PER_LEVEL) as u32;

fn file(handle: u32) -> Option<Rc<dyn HostFile>> {
    // u32::MAX: closed.
    (handle != u32::MAX).then(|| Rc::new(JsFile::new(handle)) as Rc<dyn HostFile>)
}

fn new_shell(cfg: &[u8]) -> Option<Shell> {
    let mut r = Reader { data: cfg };
    let cwd = r.bytes()?.to_vec();
    let mut env = Vec::new();
    for _ in 0..r.u32()? {
        let k = r.bytes()?.to_vec();
        let v = r.bytes()?.to_vec();
        env.push((k, v));
    }
    let (stdin, stdout, stderr) = (r.u32()?, r.u32()?, r.u32()?);
    let max_depth = match r.u32()? {
        0 => MAX_DEPTH,
        n => n.min(MAX_DEPTH),
    };
    let mut config = ShellConfig::new(Rc::new(JsHost));
    // No programs exist in wasm: every utility runs in-process (D1080).
    config.utilities = psh_utils::policy(true);
    config.cwd = cwd;
    config.env = env;
    config.stdin = file(stdin);
    config.stdout = file(stdout);
    config.stderr = file(stderr);
    config.max_depth = max_depth;
    Some(Shell::new(config))
}

export! {
    /// Creates a shell from the `len` input bytes: cwd, the environment (a count, then
    /// name and value), the stdin, stdout and stderr handles (`0xffffffff`: closed) and
    /// the nesting limit (0: the module's maximum). Returns the shell's id, or 0 for a
    /// malformed config.
    fn psh_shell_new(len: usize) -> u32 {
        let cfg = take_input(len);
        let Some(shell) = new_shell(&cfg) else {
            return 0;
        };
        SHELLS.with(|s| {
            let mut shells = s.borrow_mut();
            let id = match shells.iter().position(Option::is_none) {
                Some(i) => i,
                None => {
                    shells.push(None);
                    shells.len() - 1
                }
            };
            let sid = id as u32 + 1;
            shell.set_waker(Waker::from(Arc::new(WakeShell(sid))));
            shells[id] = Some(Entry {
                shell,
                running: false,
            });
            sid
        })
    }
}

/// Runs `f` on a shell taken out of the table, so that calls back into the module during
/// `f` (answers, wakes) never find the table borrowed.
fn with_shell<R>(id: u32, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
    let idx = (id as usize).checked_sub(1)?;
    let mut shell = SHELLS.with(|s| s.borrow_mut().get_mut(idx)?.take())?;
    let r = f(&mut shell);
    SHELLS.with(|s| s.borrow_mut()[idx] = Some(shell));
    Some(r)
}

export! {
    /// The maximum nesting limit this module allows.
    fn psh_max_depth() -> u32 {
        MAX_DEPTH
    }
}

export! {
    /// Starts a script from the `len` input bytes: the source, `$0`, the arguments (a
    /// count, then each), the command-line options (a count, then each word as
    /// `Script::options` takes them: `-e`, `+x`, `-o`, `pipefail`, `-s`; what
    /// `psh_engine::invocation::parse_args` returns). `dash_c`: run as `-c`. Returns 0, or 1 for a malformed payload or
    /// an unknown shell, 2 when a script is still running.
    fn psh_shell_start(id: u32, len: usize, dash_c: u32) -> u32 {
        let data = take_input(len);
        let mut r = Reader { data: &data };
        let parsed = (|| {
            let source = r.bytes()?.to_vec();
            let arg0 = r.bytes()?.to_vec();
            let mut args = Vec::new();
            for _ in 0..r.u32()? {
                args.push(r.bytes()?.to_vec());
            }
            let mut options = Vec::new();
            for _ in 0..r.u32()? {
                options.push(r.bytes()?.to_vec());
            }
            Some(Script {
                source,
                arg0,
                args,
                dash_c: dash_c != 0,
                options,
                stdin: false,
            })
        })();
        let Some(script) = parsed else {
            return 1;
        };
        with_shell(id, |e| {
            if e.running {
                return 2;
            }
            e.shell.start(script);
            e.running = true;
            0
        })
        .unwrap_or(1)
    }
}

export! {
    /// Runs ready work for at most `budget` safepoints: the exit status when the script
    /// finished, or -1 blocked (wait for a wake), -2 yielded (step again), -3 idle,
    /// -4 unknown shell.
    fn psh_shell_step(id: u32, budget: u32) -> i32 {
        with_shell(id, |e| match e.shell.step(budget) {
            Step::Done(status) => {
                e.running = false;
                status
            }
            Step::Blocked => STEP_BLOCKED,
            Step::Yield => STEP_YIELD,
            Step::Idle => STEP_IDLE,
        })
        .unwrap_or(STEP_BAD)
    }
}

export! {
    /// Cancels the running script (the next step ends it with 143).
    fn psh_shell_cancel(id: u32) {
        with_shell(id, |e| e.shell.control().cancel());
    }
}

export! {
    /// Frees a shell and everything it holds (its files are closed, commands abandoned).
    fn psh_shell_free(id: u32) {
        let shell = (id as usize)
            .checked_sub(1)
            .and_then(|idx| SHELLS.with(|s| s.borrow_mut().get_mut(idx)?.take()));
        drop(shell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Writer;
    use crate::ops::{Answer, complete_for_test};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A native stand-in for the JS side: stdout (handle 1) collects writes, timers
    /// answer when the test says so, everything else is "not found".
    #[test]
    fn echo_over_host_calls() {
        let out = Rc::new(RefCell::new(Vec::new()));
        let pending = Rc::new(RefCell::new(Vec::new()));
        let wakes = Rc::new(RefCell::new(0));
        let (o, p, w) = (out.clone(), pending.clone(), wakes.clone());
        sys::set_handler(move |kind, op, arg, data, _len| {
            match kind {
                sys::K_WRITE if arg == 1 => {
                    o.borrow_mut().extend_from_slice(data);
                    complete_for_test(
                        op,
                        Answer {
                            err: 0,
                            value: data.len() as f64,
                            data: vec![],
                        },
                    );
                }
                sys::K_SLEEP => p.borrow_mut().push(op),
                sys::K_WAKE => *w.borrow_mut() += 1,
                sys::K_CLOSE | sys::K_PANIC => {}
                sys::K_PID => return 42,
                _ => complete_for_test(
                    op,
                    Answer {
                        err: 2,
                        value: 0.0,
                        data: vec![],
                    },
                ),
            }
            0
        });
        let mut cfg = Writer::new();
        cfg.bytes(b"/")
            .u32(0)
            .u32(u32::MAX)
            .u32(1)
            .u32(u32::MAX)
            .u32(0);
        let shell = new_shell(&cfg.0).unwrap();
        let mut e = Entry {
            shell,
            running: false,
        };
        e.shell.set_waker(Waker::from(Arc::new(WakeShell(7))));
        e.shell.start(Script::command("echo a; echo $$"));
        let status = loop {
            match e.shell.step(1000) {
                Step::Done(s) => break s,
                Step::Yield => {}
                Step::Blocked => {
                    let ops: Vec<u32> = pending.borrow_mut().drain(..).collect();
                    assert!(!ops.is_empty(), "blocked with nothing pending");
                    for op in ops {
                        complete_for_test(op, Answer::default());
                    }
                }
                Step::Idle => panic!("idle"),
            }
        };
        assert_eq!(status, 0);
        assert_eq!(String::from_utf8_lossy(&out.borrow()), "a\n42\n");
    }
}
