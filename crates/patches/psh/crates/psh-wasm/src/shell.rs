//! The shell exports: create a shell over the JS host, start a script, step it, read its
//! result, cancel and free it. The JS side drives `psh_shell_step` from its event loop:
//! on `Blocked` it waits until the module asks it to wake the shell (`K_WAKE`), which
//! happens when one of its answers unblocks a task.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Wake, Waker};

use bytes::Bytes;
use nanocode_shell::{
    Answers, Config, Ended, HostInfo, Identity, Limit, Limits, Ran, Script, Shell, Step,
};

use crate::host::{JsCommands, JsFetch, JsFs, JsMachine, Reader, Writer};
use crate::{export, set_output, sys, take_input};

/// Step results other than done.
pub const STEP_DONE: i32 = 0;
pub const STEP_BLOCKED: i32 = -1;
pub const STEP_YIELD: i32 = -2;
pub const STEP_IDLE: i32 = -3;
/// No such shell.
pub const STEP_BAD: i32 = -4;

/// How a run ended, in its result.
const ENDED_EXITED: u32 = 0;
const ENDED_CANCELLED: u32 = 1;
const ENDED_FUEL: u32 = 2;
const ENDED_MISSING: u32 = 3;
const ENDED_OPERATIONS: u32 = 4;
const ENDED_UNANSWERED: u32 = 5;

thread_local! {
    static SHELLS: RefCell<Vec<Option<Entry>>> = const { RefCell::new(Vec::new()) };
}

struct Entry {
    shell: Shell,
    /// A script was started and has not reported `Done` yet.
    running: bool,
    /// The last finished run, until `psh_shell_result` takes it.
    ran: Option<Box<Ran>>,
}

struct WakeShell(u32);

impl Wake for WakeShell {
    fn wake(self: Arc<Self>) {
        sys::call(sys::K_WAKE, 0, self.0, &[]);
    }
}

/// The stack the module links with (build.rs) and the nesting limit it allows: each
/// level may take up to 16 KiB (psh-engine's `ShellConfig::STACK_PER_LEVEL`); a quarter
/// of the stack stays free for the rest of the call chain.
const STACK: usize = 16 * 1024 * 1024;
const STACK_PER_LEVEL: usize = 16 * 1024;
pub const MAX_DEPTH: u32 = (STACK * 3 / 4 / STACK_PER_LEVEL) as u32;

fn new_shell(cfg: &[u8]) -> Option<Shell> {
    let mut r = Reader { data: cfg };
    let cwd = r.bytes()?.to_vec();
    let mut env = Vec::new();
    for _ in 0..r.u32()? {
        let k = r.bytes()?.to_vec();
        let v = r.bytes()?.to_vec();
        env.push((k, v));
    }
    let limits = Limits {
        fuel: r.f64()? as u64,
        output: r.f64()? as u64,
        file: r.f64()? as u64,
        operations: r.f64()? as u64,
        depth: match r.u32()? {
            0 => MAX_DEPTH,
            n => n.min(MAX_DEPTH),
        },
    };
    let strict = r.u32()? != 0;
    let (uid, pid) = (r.u32()?, r.u32()?);
    let user = r.bytes()?.to_vec();
    let group = r.bytes()?.to_vec();
    let home = r.bytes()?.to_vec();
    let hostname = r.bytes()?.to_vec();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let (ostype, machtype, hosttype) = (text(r.bytes()?), text(r.bytes()?), text(r.bytes()?));
    let (release, version) = (text(r.bytes()?), text(r.bytes()?));
    let mut without = Vec::new();
    for _ in 0..r.u32()? {
        without.push(r.bytes()?.to_vec());
    }
    let names = |r: &mut Reader| {
        let mut names = std::collections::BTreeSet::new();
        for _ in 0..r.u32()? {
            names.insert(text(r.bytes()?));
        }
        Some(names)
    };
    let answers = match r.u32()? {
        0 => None,
        _ => Some(Answers {
            commands: names(&mut r)?,
            variables: names(&mut r)?,
        }),
    };
    let config = Config {
        fs: Rc::new(JsFs),
        commands: Rc::new(JsCommands),
        machine: Rc::new(JsMachine),
        identity: Identity {
            info: HostInfo {
                uid,
                euid: uid,
                groups: vec![uid],
                ppid: Some(1),
                hostname,
                ostype,
                machtype,
                hosttype,
                release,
                version,
            },
            pid,
            user,
            group,
            home,
        },
        limits,
        env,
        cwd,
        without,
        strict,
    };
    Some(make(config, answers))
}

/// The shell with every utility, or in the module without git (build.rs) with every
/// one but that.
#[cfg(not(psh_without_git))]
fn make(config: Config, answers: Option<Answers>) -> Shell {
    match answers {
        Some(answers) => Shell::with_answers(config, answers),
        None => Shell::new(config),
    }
}

#[cfg(psh_without_git)]
fn make(config: Config, answers: Option<Answers>) -> Shell {
    Shell::without_git(config, answers)
}

export! {
    /// Creates a shell from the `len` input bytes: cwd, the environment (a count, then
    /// name and value), the limits (fuel, output bytes, file bytes and file operations as
    /// f64, the nesting limit as u32 with 0 for the module's maximum), whether a missing
    /// command ends the run, the uid and pid, the user, group, home, host name, OSTYPE,
    /// MACHTYPE and HOSTTYPE, the utilities left to the host's commands (a count, then
    /// each name), and whether the shell stands in for a machine (a u32: 0 for not, else
    /// 1 and then the programs and the variables whose answers on that machine it has, a
    /// count and the names each). Returns the shell's id, or 0 for a malformed config.
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
                ran: None,
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
    /// Puts an idle shell back as `psh_shell_new` made it (its variables, functions,
    /// options and working directory), so that the JS side runs its next script in it
    /// instead of making a shell. Returns 0, or 1 for an unknown shell, 2 when a script
    /// is still running.
    fn psh_shell_renew(id: u32) -> u32 {
        with_shell(id, |e| {
            if e.running {
                return 2;
            }
            e.shell.renew();
            e.ran = None;
            0
        })
        .unwrap_or(1)
    }
}

export! {
    /// Gives a shell the JS side's network (nanocode's addition): its curl and wget
    /// then ask with `K_FETCH`. A shell that is not given one has none, and they end
    /// saying so. Returns 0, or 1 for an unknown shell.
    fn psh_shell_fetch(id: u32) -> u32 {
        with_shell(id, |e| {
            e.shell.set_fetch(Rc::new(JsFetch));
            0
        })
        .unwrap_or(1)
    }
}

export! {
    /// The maximum nesting limit this module allows.
    fn psh_max_depth() -> u32 {
        MAX_DEPTH
    }
}

export! {
    /// Starts a script from the `len` input bytes: the source, `$0`, the arguments (a
    /// count, then each), the command-line options (a count, then each word as `set` and
    /// `shopt` take them: `-e`, `+x`, `-o`, `pipefail`) and its standard input (a u32:
    /// 0 for none, which reads as the null device, else 1 and then the bytes). Returns 0,
    /// or 1 for a malformed payload or an unknown shell, 2 when a script is still running.
    fn psh_shell_start(id: u32, len: usize) -> u32 {
        let data = take_input(len);
        let mut r = Reader { data: &data };
        let parsed = (|| {
            let source = r.bytes()?.to_vec();
            let name = r.bytes()?.to_vec();
            let mut args = Vec::new();
            for _ in 0..r.u32()? {
                args.push(r.bytes()?.to_vec());
            }
            let mut options = Vec::new();
            for _ in 0..r.u32()? {
                options.push(r.bytes()?.to_vec());
            }
            let stdin = match r.u32()? {
                0 => None,
                _ => Some(Bytes::copy_from_slice(r.bytes()?)),
            };
            Some(Script {
                source,
                name,
                args,
                options,
                stdin,
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
            e.ran = None;
            0
        })
        .unwrap_or(1)
    }
}

export! {
    /// Runs ready work for at most `budget` safepoints: 0 when the script finished
    /// (`psh_shell_result` then gives its result), or -1 blocked (wait for a wake), -2
    /// yielded (step again), -3 idle, -4 unknown shell.
    fn psh_shell_step(id: u32, budget: u32) -> i32 {
        with_shell(id, |e| match e.shell.step(budget) {
            Step::Done(ran) => {
                e.running = false;
                e.ran = Some(ran);
                STEP_DONE
            }
            Step::Blocked => STEP_BLOCKED,
            Step::Yield => STEP_YIELD,
            Step::Idle => STEP_IDLE,
        })
        .unwrap_or(STEP_BAD)
    }
}

export! {
    /// Puts the last finished run into the output buffer: how it ended (0 exited, 1
    /// cancelled, 2 out of fuel, 3 a missing command, 4 too many file operations, 5
    /// something whose answer on the machine the shell stands in for is not known), the
    /// exit status, whether output was cut at its limit (u32 each), the fuel and the file
    /// operations it used (f64 each), stdout, stderr, the names of the commands that
    /// were not found (a count, then each), and what had no answer (empty unless 5).
    /// Returns 1, or 0 when there is none.
    fn psh_shell_result(id: u32) -> u32 {
        let Some(Some(ran)) = with_shell(id, |e| e.ran.take()) else {
            return 0;
        };
        let mut unanswered = String::new();
        let (ended, status) = match ran.ended {
            Ended::Exited(status) => (ENDED_EXITED, status),
            Ended::Cancelled => (ENDED_CANCELLED, 143),
            Ended::Refused(Limit::Fuel) => (ENDED_FUEL, 126),
            Ended::Refused(Limit::Missing) => (ENDED_MISSING, 127),
            Ended::Refused(Limit::Operations) => (ENDED_OPERATIONS, 126),
            Ended::Unanswered(what) => {
                unanswered = what;
                (ENDED_UNANSWERED, 127)
            }
        };
        let mut w = Writer(Vec::with_capacity(64 + ran.stdout.len() + ran.stderr.len()));
        w.u32(ended).u32(status as u32).u32(u32::from(ran.truncated));
        w.raw(&(ran.fuel as f64).to_le_bytes())
            .raw(&(ran.operations as f64).to_le_bytes());
        w.bytes(&ran.stdout).bytes(&ran.stderr);
        w.u32(ran.missing.len() as u32);
        for name in &ran.missing {
            w.bytes(name);
        }
        w.bytes(unanswered.as_bytes());
        set_output(w.0);
        1
    }
}

export! {
    /// Cancels the running script (the next step ends it).
    fn psh_shell_cancel(id: u32) {
        with_shell(id, |e| e.shell.cancel());
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
