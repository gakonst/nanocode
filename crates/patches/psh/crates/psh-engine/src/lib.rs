//! The psh interpreter: a synchronous, sans-IO state machine stepped by its embedder
//! (ARCH.md "psh-engine", D005, D1050).
//!
//! ```
//! use std::rc::Rc;
//! use psh_engine::{Script, Shell, ShellConfig, Step, testhost::TestHost};
//!
//! let host = Rc::new(TestHost::new());
//! let mut shell = Shell::new(host.config());
//! shell.start(Script::command("echo hi | cat"));
//! let status = loop {
//!     match shell.step(1000) {
//!         Step::Done(status) => break status,
//!         Step::Yield | Step::Blocked => continue,
//!         Step::Idle => unreachable!(),
//!     }
//! };
//! assert_eq!(status, 0);
//! assert_eq!(host.stdout(), b"hi\n");
//! ```
//!
//! Every side effect goes through a [`Host`]; the engine waits only through
//! `std::task::Waker`s, never by blocking the thread, and runs no threads itself. A
//! [`Shell`] is `!Send`: create and step it on one thread.

mod arith;
mod arrays;
mod builtins;
mod cond;
mod escape;
mod exec;
mod expand;
mod fd;
pub mod host;
pub mod invocation;
mod jobs;
mod now;
mod options;
mod pattern;
mod pidscan;
mod pipe;
mod sched;
mod signals;
mod state;
mod timing;
#[cfg(any(unix, windows))]
pub mod std_host;
pub mod testhost;
#[cfg(test)]
mod tests;
pub mod util;
mod vars;

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::Waker;

use psh_syntax::{LineIndex, Options, Parser};

pub use host::{Host, HostFile};

/// Whether `name` is a builtin of the shell (one of bash's: psh runs it or stops at it),
/// for an embedder that decides from a script's text where to run it.
pub fn is_builtin(name: &[u8]) -> bool {
    builtins::find(name).is_some() || builtins::BASH_BUILTINS.contains(&name)
}

/// Whether `source` reads `$$` or `BASHPID` (also in the `bash -c` strings and backquotes
/// it holds), for an embedder whose shells share its process id.
pub fn uses_pid(source: &[u8]) -> bool {
    pidscan::uses_pid(source)
}

use fd::{FdObj, FdTable};
use sched::{CANCEL, Join, PAUSE, RunEnd, Sched, Shared};
use state::{Code, Core, Unwind, Var, Vars, Vp};

/// What one call of [`Shell::step`] ended with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The script finished with this status. Reported once; then `Idle`.
    Done(i32),
    /// Everything waits on the host (or the shell is paused). The waker given to
    /// [`Shell::set_waker`] fires when there is work again.
    Blocked,
    /// The budget is spent; call `step` again (there is work).
    Yield,
    /// No script is running.
    Idle,
}

/// Settings for a new [`Shell`].
pub struct ShellConfig {
    pub host: Rc<dyn Host>,
    /// The environment the shell starts with.
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    /// The working directory, absolute.
    pub cwd: Vec<u8>,
    /// Descriptors 0, 1 and 2; `None` is closed.
    pub stdin: Option<Rc<dyn HostFile>>,
    pub stdout: Option<Rc<dyn HostFile>>,
    pub stderr: Option<Rc<dyn HostFile>>,
    /// The nesting limit for subshells and substitutions (D1050). Each level costs
    /// native stack while it is polled; see [`ShellConfig::STACK_PER_LEVEL`].
    pub max_depth: u32,
    /// What happens to background jobs (`cmd &`) still running when the script ends.
    /// `None`: they are released and outlive the shell, as in bash. `Some(grace)`: the
    /// shell waits up to `grace` for them, then abandons them (TERM, KILL): for
    /// embedders that must leave no process behind (D1051).
    pub exit_grace: Option<std::time::Duration>,
    /// The in-process utilities (psh-utils' registry) and their off switches; empty by
    /// default: every command name not a function or builtin is a program (D1080).
    pub utilities: util::UtilPolicy,
    /// A command named `bash` or `sh` runs in psh, as a new shell (D1410); `false`
    /// sends both names to `PATH` again. On by default.
    pub nested_shells: bool,
    /// The program this shell runs in (the `psh` binary), if any: a nested `bash` never
    /// falls through to it, should it be installed as `bash` (D1413).
    pub own_program: Option<Vec<u8>>,
}

impl ShellConfig {
    /// Native stack an embedder should budget per nesting level: a function call
    /// measured about 3 KiB in release and 15 KiB in debug builds
    /// (`examples/stackprobe.rs`, D1050, D1059).
    pub const STACK_PER_LEVEL: usize = 16 * 1024;
    /// The default nesting limit: 1000 levels need up to 16 MiB of stack. On wasm32
    /// (1 MiB stack by default) use about 200, or link with a larger stack.
    pub const DEFAULT_MAX_DEPTH: u32 = 1000;

    /// A config with closed stdio, an empty environment and `/` as the working
    /// directory.
    pub fn new(host: Rc<dyn Host>) -> ShellConfig {
        ShellConfig {
            host,
            env: Vec::new(),
            cwd: b"/".to_vec(),
            stdin: None,
            stdout: None,
            stderr: None,
            max_depth: ShellConfig::DEFAULT_MAX_DEPTH,
            exit_grace: None,
            utilities: util::UtilPolicy::default(),
            nested_shells: true,
            own_program: None,
        }
    }
}

/// A script to run.
pub struct Script {
    /// The script's bytes; need not be UTF-8 (parsed byte-exactly).
    pub source: Vec<u8>,
    /// `$0`, also the name in front of messages.
    pub arg0: Vec<u8>,
    /// `$1`…
    pub args: Vec<Vec<u8>>,
    /// Run as `-c`: syntax errors say `-c:`.
    pub dash_c: bool,
    /// Options from the command line, as `set` and `shopt` take them: `-e`, `+x`,
    /// `-o NAME`, `-O NAME` (a `shopt` option), `+O NAME`.
    pub options: Vec<Vec<u8>>,
    /// The script is descriptor 0, read one complete command at a time as it runs, so
    /// its commands can read the rest of stdin (`psh` with no script operand, `-s`;
    /// D1414); `source` is not used.
    pub stdin: bool,
}

impl Script {
    /// `psh -c SOURCE`.
    pub fn command(source: &str) -> Script {
        Script {
            source: source.as_bytes().to_vec(),
            arg0: b"psh".to_vec(),
            args: Vec::new(),
            dash_c: true,
            options: Vec::new(),
            stdin: false,
        }
    }
}

/// Lets another thread cancel or pause a running shell. Cheap to clone, `Send` and
/// `Sync`.
#[derive(Clone)]
pub struct ControlHandle {
    shared: Arc<Shared>,
}

impl ControlHandle {
    /// Stops the run: at the next step every vproc gets a virtual TERM (no traps yet:
    /// the default action, termination) and the run ends with status 143. Externals
    /// are abandoned (TERM, then KILL after the host's grace period).
    pub fn cancel(&self) {
        self.shared.control.fetch_or(CANCEL, Ordering::SeqCst);
        self.shared.wake_outer();
    }

    /// Whether [`ControlHandle::cancel`] was called and the run has not ended yet
    /// (nanocode's addition): what a host command that computes inside one step asks,
    /// since the step cannot end under it.
    pub fn cancelled(&self) -> bool {
        self.shared.control.load(Ordering::SeqCst) & CANCEL != 0
    }

    /// Sends signal `sig` (Linux numbering, as `kill -l`) to the shell (D1284): at its
    /// next safepoint the shell runs the trap for it, ignores it (`trap '' SIG`), or
    /// takes the default action: the EXIT trap, then the run ends with `128+sig`. Unlike
    /// `cancel`, it waits for the command running in the foreground to finish, as bash
    /// does. For `kill` on the shell and for an embedder's own signal handling.
    pub fn signal(&self, sig: i32) {
        if (1..64).contains(&sig) {
            self.shared.signals.fetch_or(1u64 << sig, Ordering::SeqCst);
            self.shared.wake_outer();
        }
    }

    /// Wakes the embedder's waker without changing anything: after a signal handler set
    /// a bit in [`ControlHandle::signal_word`] itself (calling `signal` then would
    /// deliver the signal a second time if the shell took the first already).
    pub fn wake(&self) {
        self.shared.wake_outer();
    }

    /// The pending-signal mask [`ControlHandle::signal`] sets (bit n = Linux signal n):
    /// an atomic or into it is that call without the wakeup, and async-signal-safe (a
    /// standalone shell's signal handler sets it; its driver then calls `signal`, D1903).
    pub fn signal_word(&self) -> &std::sync::atomic::AtomicU64 {
        &self.shared.signals
    }

    /// Stops at the next safepoint; `step` returns `Blocked` until `resume`.
    pub fn pause(&self) {
        self.shared.control.fetch_or(PAUSE, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.shared.control.fetch_and(!PAUSE, Ordering::SeqCst);
        self.shared.wake_outer();
    }
}

/// A shell: its state, the scheduler of its vprocs, and the script running.
pub struct Shell {
    core: Rc<Core>,
    shared: Arc<Shared>,
    /// The main vproc's state between runs.
    vp: Option<Vp>,
    /// The state the shell was made with ([`Shell::renew`]); it shares the variables.
    fresh: Vp,
    /// The state before the current run, restored if it is cancelled.
    before: Option<Vp>,
    main: Option<Join<(i32, Vp)>>,
    /// The script ended with this status; its background jobs get until the timer
    /// (`exit_grace`) to finish.
    draining: Option<(i32, Join<()>)>,
    exit_grace: Option<std::time::Duration>,
    /// `Rc` inside: a shell stays on the thread that made it.
    _not_send: PhantomData<*const ()>,
}

impl Shell {
    pub fn new(cfg: ShellConfig) -> Shell {
        let shared = Arc::new(Shared::new());
        let core = Rc::new(Core {
            host: cfg.host.clone(),
            sched: Rc::new(Sched::new(shared.clone())),
            max_depth: cfg.max_depth,
            jobs: Cell::new(0),
            release_on_drop: Cell::new(false),
            died_of: Cell::new(None),
            fg_signal: Cell::new(None),
            utilities: cfg.utilities.clone(),
            nested_shells: cfg.nested_shells,
            own_program: cfg.own_program.clone(),
            rng: Cell::new(0),
            seconds_base: Cell::new(cfg.host.now().as_nanos() as i128),
            forked: Cell::new(None),
        });
        let (vars, foreign) = initial_vars(&*cfg.host, &cfg.env, &cfg.cwd);
        let opts = options::inherited(&vars);
        let mut fds = FdTable::default();
        for (n, f) in [(0, cfg.stdin), (1, cfg.stdout), (2, cfg.stderr)] {
            if let Some(f) = f {
                fds.set(n, Rc::new(FdObj::File(f)));
            }
        }
        let vp = Vp {
            core: core.clone(),
            vars,
            foreign_env: Rc::new(foreign),
            arg0: b"psh".to_vec(),
            args: Vec::new(),
            cwd: cfg.cwd,
            fds,
            status: 0,
            line: 0,
            depth: 0,
            last_bg: None,
            jobs: Default::default(),
            job: None,
            exec_tail: None,
            exec_now: false,
            in_subst: false,
            funcs: Rc::new(Default::default()),
            frames: Vec::new(),
            loops: 0,
            getopts: (0, 0),
            subst_fds: Vec::new(),
            var_fds: Vec::new(),
            stage_subshell: false,
            last_args: Vec::new(),
            subshell_level: 0,
            arith_unbound: false,
            exported_funcs: Rc::new(Vec::new()),
            traced_funcs: Rc::new(Vec::new()),
            bashpid: Some(cfg.host.pid()),
            shell_pid: Some(cfg.host.pid()),
            script_args: Vec::new(),
            aliases: psh_syntax::Aliases::new(),
            expand_aliases: false,
            calls: Vec::new(),
            hash: Default::default(),
            disabled: Default::default(),
            opts,
            no_errexit: false,
            bad_assign: false,
            traps: Rc::new(Default::default()),
            in_trap: None,
            signal_trap: false,
            top: true,
            fork_job: false,
            sigs: state::Sigs::Shell,
            xtrace_level: 1,
            umask: None,
            dirstack: Vec::new(),
            rlimits: Vec::new(),
        };
        let mut vp = vp;
        options::sync_vars(&mut vp.vars, &vp.opts);
        vp.vars.settle();
        Shell {
            core,
            shared,
            fresh: vp.clone(),
            vp: Some(vp),
            before: None,
            main: None,
            draining: None,
            exit_grace: cfg.exit_grace,
            _not_send: PhantomData,
        }
    }

    /// Puts the shell back as it was made: the variables, functions, options, working
    /// directory and descriptors of its `ShellConfig`, with `SECONDS` and `RANDOM`
    /// starting over. An embedder that runs unrelated scripts one after the other
    /// reuses one shell instead of making one per script. Panics if a script is running.
    pub fn renew(&mut self) {
        assert!(
            self.main.is_none() && self.draining.is_none(),
            "a script is still running"
        );
        let mut vp = self.fresh.clone();
        // The job table is shared by every copy of a state: a renewed shell has its own.
        vp.jobs = Default::default();
        self.vp = Some(vp);
        self.before = None;
        self.core.jobs.set(0);
        self.core.rng.set(0);
        self.core
            .seconds_base
            .set(self.core.host.now().as_nanos() as i128);
    }

    /// The handle to cancel or pause this shell from any thread.
    pub fn control(&self) -> ControlHandle {
        ControlHandle {
            shared: self.shared.clone(),
        }
    }

    /// The waker to wake when a `Blocked` shell can make progress.
    pub fn set_waker(&self, waker: Waker) {
        self.shared.set_waker(waker);
    }

    /// Starts a script. Clears earlier cancel and pause requests, so a late cancel
    /// never reaches the next run. Panics if a script is still running.
    pub fn start(&mut self, script: Script) {
        assert!(self.main.is_none(), "a script is already running");
        self.shared.control.store(0, Ordering::SeqCst);
        self.shared.signals.store(0, Ordering::SeqCst);
        self.core.sched.forget_signal_waiters();
        self.core.died_of.set(None);
        let mut vp = self.vp.take().expect("shell state");
        // The copy kept for a cancelled run shares the variables.
        vp.vars.settle();
        self.before = Some(vp.clone());
        // `$_` starts as `$0` unless the environment had one.
        if vp.vars.get("_").is_none() {
            vp.vars.insert("_", Var::scalar(script.arg0.clone()));
        }
        vp.arg0 = script.arg0;
        vp.script_args = script.args.clone();
        vp.args = script.args;
        vp.opts.dash_c = script.dash_c;
        builtins::apply_invocation_options(&mut vp, &script.options);
        let input = if script.stdin {
            Input::Stdin
        } else {
            Input::Text(script.source)
        };
        let fut = run_script(vp, input);
        self.main = Some(self.core.sched.spawn(fut));
    }

    /// Runs ready work for at most `budget` safepoints (about one command each).
    pub fn step(&mut self, budget: u32) -> Step {
        // Signals sent to the shell end the waits that give way to them (D1903).
        if self.core.sched.has_signals() {
            self.core.sched.wake_signal_waiters();
        }
        loop {
            if let Some((status, timer)) = &self.draining {
                let status = *status;
                let ctl = self.shared.control.load(Ordering::SeqCst);
                let sched = &self.core.sched;
                // Done when only the timer is left, or it fired.
                let end = if ctl & CANCEL != 0 {
                    RunEnd::Done
                } else {
                    sched.run(budget, &|| timer.is_done() || sched.task_count() <= 1)
                };
                match end {
                    RunEnd::Done => {
                        self.draining = None;
                        self.core.sched.drop_all();
                        self.shared.control.store(0, Ordering::SeqCst);
                        return Step::Done(status);
                    }
                    RunEnd::Yield => return Step::Yield,
                    RunEnd::Blocked => return Step::Blocked,
                    RunEnd::Control => continue,
                }
            }
            // A forked copy of the shell (D1900): the run is the background job it was
            // made for, and ends with it. What the job left running outlives the copy,
            // as a subshell's children do.
            if self.core.forked.get().is_some() {
                let ctl = self.shared.control.load(Ordering::SeqCst);
                if ctl & CANCEL != 0 {
                    return self.finish_cancelled();
                }
                if ctl & PAUSE != 0 {
                    return Step::Blocked;
                }
                let forked = &self.core.forked;
                let ended = || matches!(forked.get(), Some(Some(_)));
                let end = if ended() {
                    RunEnd::Done
                } else {
                    self.core.sched.run(budget, &ended)
                };
                match end {
                    RunEnd::Done => {
                        let status = forked.get().flatten().unwrap_or(0);
                        self.main = None;
                        self.core.sched.run(budget, &|| false);
                        self.core.release_on_drop.set(true);
                        self.core.sched.drop_all();
                        self.core.release_on_drop.set(false);
                        return Step::Done(status);
                    }
                    RunEnd::Yield => return Step::Yield,
                    RunEnd::Blocked => return Step::Blocked,
                    RunEnd::Control => continue,
                }
            }
            let Some(main) = &self.main else {
                return Step::Idle;
            };
            let ctl = self.shared.control.load(Ordering::SeqCst);
            if ctl & CANCEL != 0 {
                return self.finish_cancelled();
            }
            if ctl & PAUSE != 0 {
                return Step::Blocked;
            }
            let forked = &self.core.forked;
            let end = self
                .core
                .sched
                .run(budget, &|| main.is_done() || forked.get().is_some());
            // A task forked: this is the copy, and its job the run.
            if forked.get().is_some() {
                continue;
            }
            match end {
                RunEnd::Done => {
                    let (status, vp) = self
                        .main
                        .take()
                        .and_then(|m| m.try_take())
                        .expect("main result");
                    self.vp = Some(vp);
                    // Give detached vprocs (`cmd &`) what is ready.
                    self.core.sched.run(budget, &|| false);
                    // One of them forked: this is the copy, and its job the run.
                    if self.core.forked.get().is_some() {
                        continue;
                    }
                    if let Some(grace) = self.exit_grace
                        && self.core.sched.task_count() > 0
                    {
                        let timer = self.core.host.sleep(grace);
                        let join = self.core.sched.spawn(timer);
                        self.draining = Some((status, join));
                        continue;
                    }
                    // Release what still runs (bash leaves background jobs running).
                    self.core.release_on_drop.set(true);
                    self.core.sched.drop_all();
                    self.core.release_on_drop.set(false);
                    return Step::Done(status);
                }
                RunEnd::Yield => return Step::Yield,
                RunEnd::Blocked => return Step::Blocked,
                RunEnd::Control => continue,
            }
        }
    }

    fn finish_cancelled(&mut self) -> Step {
        self.main = None;
        self.core.sched.drop_all();
        self.shared.control.store(0, Ordering::SeqCst);
        // The main vproc's state went with its task: the next run starts from the
        // state before this one.
        self.vp = self.before.take();
        Step::Done(128 + host::SIGTERM)
    }

    /// The signal (Linux numbering) whose default action ended the last run, if one did
    /// (D1903): a standalone shell dies by it, so its parent sees what bash shows.
    pub fn killed_by(&self) -> Option<i32> {
        self.core.died_of.get()
    }

    /// Whether this process is a forked copy of the shell that ran one background job
    /// (`Host::prepare_fork`): the embedder's process ends with the run, since no
    /// thread but this one exists in it.
    pub fn forked(&self) -> bool {
        self.core.forked.get().is_some()
    }

    /// The commands and loop iterations this shell has run in all its steps (a utility's
    /// own safepoints are not counted): an embedder that meters a run charges the
    /// difference between two readings (nanocode-shell's fuel).
    pub fn spent(&self) -> u64 {
        self.core.sched.spent()
    }

    /// The number of live tasks (vprocs and helpers), for leak tests.
    pub fn task_count(&self) -> usize {
        self.core.sched.task_count()
    }
}

/// Environment pairs, names and values as bytes.
type Env = Vec<(Vec<u8>, Vec<u8>)>;

/// The variables a new shell starts with, from its environment (D1329): what
/// [`Shell::new`] sets up, and what a shell psh starts afresh gets from its exported
/// variables (D1300, D1410). Names that are not shell names stay in the second list,
/// passed on to externals unchanged.
pub(crate) fn initial_vars(host: &dyn Host, env: &[(Vec<u8>, Vec<u8>)], cwd: &[u8]) -> (Vars, Env) {
    let mut vars = Vars::default();
    let mut foreign = Vec::new();
    for (k, v) in env {
        match std::str::from_utf8(k) {
            Ok(name) if state::is_name(k) => vars.insert(
                name,
                Var {
                    exported: true,
                    ..Var::scalar(v.clone())
                },
            ),
            _ => foreign.push((k.clone(), v.clone())),
        }
    }
    vars.insert("IFS", Var::scalar(b" \t\n".to_vec()));
    // Computed when read (D1263).
    for (name, d) in [
        ("RANDOM", vars::Dynamic::Random),
        ("SRANDOM", vars::Dynamic::Srandom),
        ("SECONDS", vars::Dynamic::Seconds),
        ("EPOCHSECONDS", vars::Dynamic::EpochSeconds),
        ("EPOCHREALTIME", vars::Dynamic::EpochRealtime),
        ("BASHPID", vars::Dynamic::BashPid),
        ("BASH_SUBSHELL", vars::Dynamic::BashSubshell),
        ("FUNCNAME", vars::Dynamic::FuncName),
        ("BASH_SOURCE", vars::Dynamic::BashSource),
        ("BASH_LINENO", vars::Dynamic::BashLineno),
        ("BASH_ARGV", vars::Dynamic::BashArgv),
        ("BASH_ARGC", vars::Dynamic::BashArgc),
        ("GROUPS", vars::Dynamic::Groups),
        ("BASH_ARGV0", vars::Dynamic::BashArgv0),
        ("BASH_ALIASES", vars::Dynamic::BashAliases),
        ("BASH_MONOSECONDS", vars::Dynamic::BashMonoseconds),
        ("LINENO", vars::Dynamic::Lineno),
        ("DIRSTACK", vars::Dynamic::Dirstack),
    ] {
        let v = vars.entry(name);
        v.dynamic = Some(d);
        // The attributes `declare -p` shows for them in bash.
        v.integer = matches!(
            d,
            vars::Dynamic::Random | vars::Dynamic::Srandom | vars::Dynamic::BashPid
        );
        if matches!(
            d,
            vars::Dynamic::FuncName
                | vars::Dynamic::BashSource
                | vars::Dynamic::BashLineno
                | vars::Dynamic::BashArgv
                | vars::Dynamic::BashArgc
                | vars::Dynamic::Groups
        ) {
            v.value = vars::Value::Indexed(Default::default());
            v.declared_only = true;
        }
        if d == vars::Dynamic::Dirstack {
            v.value = vars::Value::Indexed(Default::default());
        }
        if d == vars::Dynamic::BashAliases {
            v.value = vars::Value::Assoc(Default::default());
        }
    }
    // Who and where the shell runs, from the host (D1328). As in bash: UID and
    // EUID (readonly integers) and the names below give way to the environment's;
    // PPID never does; GROUPS is computed, so assignments to it do nothing.
    let info = host.info();
    let ro_int = |n: u32| Var {
        integer: true,
        readonly: true,
        ..Var::scalar(n.to_string().into_bytes())
    };
    for (name, n) in [("UID", info.uid), ("EUID", info.euid)] {
        if vars.get(name).is_none() {
            vars.insert(name, ro_int(n));
        }
    }
    if let Some(p) = info.ppid {
        vars.insert("PPID", ro_int(p));
    }
    for (name, v) in [
        ("HOSTNAME", info.hostname.clone()),
        ("OSTYPE", info.ostype.clone().into_bytes()),
        ("MACHTYPE", info.machtype.clone().into_bytes()),
        ("HOSTTYPE", info.hosttype.clone().into_bytes()),
    ] {
        if vars.get(name).is_none() {
            vars.insert(name, Var::scalar(v));
        }
    }
    // psh's compatibility target; scripts gate bash-isms on these (D1264).
    vars.insert("BASH_VERSION", Var::scalar(b"5.3.9(1)-release".to_vec()));
    let mut versinfo = std::collections::BTreeMap::new();
    for (i, v) in ["5", "3", "9", "1", "release", &info.machtype]
        .iter()
        .enumerate()
    {
        versinfo.insert(i as i64, v.as_bytes().to_vec());
    }
    vars.insert(
        "BASH_VERSINFO",
        Var {
            value: vars::Value::Indexed(versinfo),
            readonly: true,
            ..Var::default()
        },
    );
    // getopts' state, as bash starts it.
    vars.insert(
        "OPTIND",
        Var {
            integer: true,
            ..Var::scalar(b"1".to_vec())
        },
    );
    vars.insert("OPTERR", Var::scalar(b"1".to_vec()));
    // What bash sets when the environment has none (D1329). Prompts belong to
    // interactive shells: a non-interactive bash drops inherited PS1 and PS2.
    vars.remove("PS1");
    vars.remove("PS2");
    let defaults: [(&str, &[u8]); 3] = [
        ("PS4", b"+ "),
        ("TERM", b"dumb"),
        ("COMP_WORDBREAKS", b" \t\n\"'@><=;|&(:"),
    ];
    for (name, v) in defaults {
        if vars.get(name).is_none() {
            vars.insert(name, Var::scalar(v.to_vec()));
        }
    }
    // bash's default PATH; on Windows the host's PATH conventions differ.
    if vars.get("PATH").is_none() && !cfg!(windows) {
        vars.insert(
            "PATH",
            Var::scalar(b"/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin:.".to_vec()),
        );
    }
    vars.insert(
        "PSH_VERSION",
        Var::scalar(env!("CARGO_PKG_VERSION").as_bytes().to_vec()),
    );
    let pwd = vars.entry("PWD");
    pwd.value = vars::Value::Str(cwd.to_vec());
    pwd.exported = true;
    let shlvl = vars
        .value("SHLVL")
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let lvl = vars.entry("SHLVL");
    lvl.value = vars::Value::Str((shlvl + 1).to_string().into_bytes());
    lvl.exported = true;
    let old = vars.entry("OLDPWD");
    old.exported = true;
    (vars, foreign)
}

/// Where a script comes from.
pub(crate) enum Input {
    Text(Vec<u8>),
    /// Descriptor 0, read one complete command at a time (D1414).
    Stdin,
}

/// The main vproc: runs the script, then the EXIT trap (D1283).
pub(crate) async fn run_script(vp: Vp, input: Input) -> (i32, Vp) {
    let (mut status, mut vp) = match input {
        Input::Text(source) => run_script_lines(vp, source).await,
        Input::Stdin => run_stdin_lines(vp).await,
    };
    // A signal that came with the last command still acts (D1903).
    if let Err(u) = builtins::trap::take_signals(&mut vp).await {
        status = u.status(status);
    }
    let status = builtins::trap::exit_trap(&mut vp, status).await;
    vp.status = status;
    (status, vp)
}

/// Parses and runs the script one top-level line at a time.
async fn run_script_lines(mut vp: Vp, source: Vec<u8>) -> (i32, Vp) {
    let lines = Rc::new(LineIndex::from_bytes(&source));
    let mut parser = Parser::from_bytes(&source, Options::default());
    loop {
        // Bash expands aliases while it reads each line, with the table it has then.
        parser.set_aliases(vp.expand_aliases.then_some(&vp.aliases));
        // `shopt -s extglob` takes effect from the next line on, as in bash (D1043).
        parser.set_options(options::parser_options(&vp.opts));
        let start = parser.offset();
        let Some(chunk) = parser.next_chunk() else {
            break;
        };
        let end = parser.offset().min(source.len());
        if let Some(st) = run_chunk(&mut vp, chunk, &source, &lines, start..end).await {
            return (st, vp);
        }
    }
    let st = vp.status;
    (st, vp)
}

/// Reads the script from descriptor 0 as it runs it, one complete top-level line at a
/// time, as bash does (D1414): the commands can read the rest of stdin themselves
/// (`read`, `cat`), and the commands before a syntax error have run. A line is read one
/// byte at a time, so nothing past it is taken from a pipe; more lines are read while
/// the parser says the input is incomplete or a here-document lacks its delimiter.
async fn run_stdin_lines(mut vp: Vp) -> (i32, Vp) {
    // The bytes read and not yet run, from the start of a line, and the lines before.
    let mut text: Vec<u8> = Vec::new();
    let mut base = 0usize;
    let mut eof = false;
    loop {
        if !eof && !text.contains(&b'\n') {
            match read_line(&mut vp, &mut text).await {
                Ok(more) => eof = !more,
                Err(st) => return (st, vp),
            }
        }
        if text.is_empty() {
            break;
        }
        // Blank lines in front keep line numbers (`line N`, `$LINENO`, the parser's
        // own `on line N`) those of the whole script.
        let mut buf = vec![b'\n'; base];
        buf.extend_from_slice(&text);
        let mut parser = Parser::from_bytes(&buf, options::parser_options(&vp.opts));
        parser.set_aliases(vp.expand_aliases.then_some(&vp.aliases));
        let chunk = parser.next_chunk();
        // The input so far ends inside a command: a construct the parser calls
        // incomplete, an error at the very end (`[[ (` before its newline), a
        // here-document without its delimiter, or a line continuation.
        let at_end = |e: &psh_syntax::ParseError| {
            let len = buf
                .iter()
                .rposition(|b| !b.is_ascii_whitespace())
                .map_or(0, |i| i + 1);
            e.span.start as usize >= len
        };
        let more = match &chunk {
            Some(Err(e)) => e.incomplete || at_end(e),
            Some(Ok(ast)) => {
                !heredocs_terminated(ast) || (parser.offset() >= buf.len() && continues(&text))
            }
            None => continues(&text),
        };
        if more && !eof {
            match read_line(&mut vp, &mut text).await {
                Ok(m) => eof = !m,
                Err(st) => return (st, vp),
            }
            continue;
        }
        let Some(chunk) = chunk else {
            // Only blank lines and comments.
            if eof {
                break;
            }
            base += text.iter().filter(|&&b| b == b'\n').count();
            text.clear();
            continue;
        };
        let end = parser.offset().min(buf.len());
        let lines = Rc::new(LineIndex::from_bytes(&buf));
        if let Some(st) = run_chunk(&mut vp, chunk, &buf, &lines, base..end).await {
            return (st, vp);
        }
        let used = end - base;
        base += text[..used].iter().filter(|&&b| b == b'\n').count();
        text.drain(..used);
    }
    let st = vp.status;
    (st, vp)
}

/// Whether the text ends in a line continuation (a backslash that is not escaped, then
/// the newline). Also in a comment, where it is none: reading one more line then costs
/// nothing but the line.
fn continues(text: &[u8]) -> bool {
    let Some(body) = text.strip_suffix(b"\n") else {
        return false;
    };
    let body = body.strip_suffix(b"\r").unwrap_or(body);
    body.iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

/// Whether every here-document of the chunk found its delimiter line.
fn heredocs_terminated(ast: &psh_syntax::Ast) -> bool {
    ast.cmd_ids().all(|id| {
        ast.cmd(id).redirects.iter().all(|r| match &r.target {
            psh_syntax::RedirTarget::HereDoc(h) => ast.heredoc(*h).terminated,
            psh_syntax::RedirTarget::Word(_) => true,
        })
    })
}

/// Reads one line of the script from descriptor 0 into `text`, a byte at a time; false
/// at the end of the input. A read error ends the input, as bash's does. Waits give way
/// to the shell's signals; one that ends the shell ends it with its status (`Err`).
async fn read_line(vp: &mut Vp, text: &mut Vec<u8>) -> Result<bool, i32> {
    loop {
        let Some(fd) = vp.fd(0) else {
            return Ok(false);
        };
        let mut b = [0u8];
        let got = signals::or_signal(vp, signals::Gives::Fatal, fd.read(&mut b)).await;
        match got {
            Ok(Ok(1)) => {
                text.push(b[0]);
                if b[0] == b'\n' {
                    return Ok(true);
                }
            }
            Ok(_) => return Ok(false),
            Err(_) => {
                if let Err(u) = builtins::trap::take_signals(vp).await {
                    return Err(u.status(vp.status));
                }
            }
        }
    }
}

/// Runs one parsed chunk of a script (`echo`: its text, for `set -v`), or reports its
/// syntax error. `Some(status)`: the script ends.
async fn run_chunk(
    vp: &mut Vp,
    chunk: Result<psh_syntax::Ast, psh_syntax::ParseError>,
    source: &[u8],
    lines: &Rc<LineIndex>,
    echo: std::ops::Range<usize>,
) -> Option<i32> {
    // `set -v`: the input is echoed as it is read.
    if vp.opts.get(options::SetOpt::Verbose) {
        vp.error_raw(&source[echo]).await;
    }
    match chunk {
        // `set -n`: read, but do not run.
        Ok(_) if vp.opts.get(options::SetOpt::Noexec) => None,
        Ok(ast) => {
            let root = ast.root();
            let code = Rc::new(Code {
                ast: Arc::new(ast),
                lines: lines.clone(),
                line_base: 0,
                arith: Default::default(),
            });
            match exec::run_list(vp, &code, root).await {
                Ok(s) => {
                    vp.status = s;
                    None
                }
                Err(u) => {
                    let st = u.status(vp.status);
                    vp.status = st;
                    matches!(u, Unwind::Exit(_)).then_some(st)
                }
            }
        }
        Err(e) => {
            let line = lines.line(e.span.start);
            vp.line = line;
            let dash_c = if vp.opts.dash_c { "-c: " } else { "" };
            let name = vp.msg_name();
            let mut msg = Vec::new();
            for l in e.message.split('\n') {
                msg.extend_from_slice(format!("{name}: {dash_c}line {line}: {l}\n").as_bytes());
            }
            if e.message
                .lines()
                .last()
                .is_some_and(|l| l.starts_with("syntax error near"))
            {
                let text = source_line(source, line);
                msg.extend_from_slice(format!("{name}: {dash_c}line {line}: `").as_bytes());
                msg.extend_from_slice(text);
                msg.extend_from_slice(b"'\n");
            }
            vp.error_raw(&msg).await;
            vp.status = 2;
            Some(2)
        }
    }
}

fn source_line(source: &[u8], line: u32) -> &[u8] {
    let l = source
        .split(|&b| b == b'\n')
        .nth(line as usize - 1)
        .unwrap_or(b"");
    l.strip_suffix(b"\r").unwrap_or(l)
}

/// Runs a command string on the test host's empty fs: for quick checks and doc tests.
#[doc(hidden)]
pub fn run_on_test_host(source: &str) -> (i32, Vec<u8>, Vec<u8>) {
    let host = Rc::new(testhost::TestHost::new());
    let mut shell = Shell::new(host.config());
    shell.start(Script::command(source));
    let status = loop {
        match shell.step(1000) {
            Step::Done(s) => break s,
            Step::Yield | Step::Blocked => {}
            Step::Idle => break 0,
        }
    };
    (status, host.stdout(), host.stderr())
}
