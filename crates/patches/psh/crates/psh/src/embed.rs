//! psh inside an embedder's own process (Unix): `contained` says whether a script is
//! one the shell runs without starting a program, and `start` runs a script on an
//! engine thread of its own with the embedder's sinks as its output.
//!
//! For a host that runs each command of an agent in a process and wants the commands
//! that are only builtins and psh's utilities (`cat f | head`, `grep -rn x src | wc -l`)
//! without one. Such a run shares the embedder's process: its memory, its descriptor
//! table and its fate. The shell's `$$` is not the process's, so `kill` cannot reach the
//! embedder, and a program a utility falls through to leads a session of its own and is
//! killed when the run is stopped or ends.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use psh_classify::Category;
use psh_engine::host::{FileKind, SIGINT};
use psh_engine::std_host::{drive, engine_thread};
use psh_engine::util::Mode;
use psh_engine::{ControlHandle, HostFile, Script, Shell, ShellConfig};
use psh_os::unix::{RunKiller, UnixConfig, UnixFile, UnixHost};

/// Where a stream of the script goes. Called on the engine thread with each write; an
/// error is the script's write error (`BrokenPipe` ends the writer as SIGPIPE would).
/// It may block: the script waits, as on a full pipe.
pub type Sink = Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>;

/// One script to run.
pub struct Command {
    pub script: Vec<u8>,
    /// The working directory, absolute.
    pub cwd: Vec<u8>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub stdout: Sink,
    pub stderr: Sink,
}

/// A `$$` no process has (above every pid a system gives out): with it psh-os refuses
/// to signal the embedder or its group (`UnixConfig::pid`). No contained script reads it.
const NO_PID: u32 = 99_999_999;

/// Builtins a contained script may not use: they act on the process the shell is.
const PROCESS_BUILTINS: [&str; 3] = ["exec", "kill", "suspend"];

/// Whether `script` runs entirely inside the shell: every command it can run (also in
/// `$(…)`, `bash -c` strings, `xargs` and `find -exec`) is a builtin or a utility that psh
/// always runs itself with the utilities `env` leaves on, named without a path; nothing is
/// started in the background; nothing reads the shell's process ids. What the classifier
/// cannot see through (a command name from a variable, `eval` of a non-literal, `source`,
/// a function defined in the script) is not contained.
pub fn contained(script: &str, env: &[(Vec<u8>, Vec<u8>)]) -> bool {
    let mut own = env
        .iter()
        .filter(|(name, _)| name.starts_with(b"PSH_"))
        .cloned()
        .collect();
    let (utilities, _) = crate::settings(&mut own);
    if utilities.off_all || script.contains("PPID") || psh_engine::uses_pid(script.as_bytes()) {
        return false;
    }
    let classified = psh_classify::classify(script);
    let outside = [
        Category::Network,
        Category::Build,
        Category::LongRunning,
        Category::Vcs,
        Category::Install,
        Category::Unknown,
    ];
    if classified.error.is_some() || outside.iter().any(|c| classified.categories.contains(*c)) {
        return false;
    }
    classified.commands.iter().all(|command| {
        // A command without words only assigns.
        let Some(word) = command.words.first() else {
            return true;
        };
        let Some(name) = word.value.as_deref() else {
            return false;
        };
        if name.contains('/') || PROCESS_BUILTINS.contains(&name) {
            return false;
        }
        psh_engine::is_builtin(name.as_bytes())
            || utilities.entries.iter().any(|entry| {
                entry.mode == Mode::Always
                    && entry.names.contains(&name)
                    && !(entry.destructive && utilities.off_destructive)
                    && !utilities.off_names.iter().any(|off| off == name.as_bytes())
            })
    })
}

/// A script that is running: a handle any thread may use.
#[derive(Clone)]
pub struct Running(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    /// The shell's control and the killer of what it started, once the engine thread
    /// made them.
    shell: Option<(ControlHandle, RunKiller)>,
    /// Asked for before the shell existed.
    stop: bool,
    interrupt: bool,
}

impl Running {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ctrl-C: the script's INT trap runs, or it ends with status 130.
    pub fn interrupt(&self) {
        let mut state = self.state();
        match &state.shell {
            Some((control, _)) => control.signal(SIGINT),
            None => state.interrupt = true,
        }
    }

    /// Ends the run now (status 143): no trap runs, and every program it started is
    /// killed.
    pub fn stop(&self) {
        let mut state = self.state();
        match &state.shell {
            Some((control, killer)) => {
                control.cancel();
                killer.kill_all();
            }
            None => state.stop = true,
        }
    }
}

/// A sink as one of the shell's descriptors: a pipe that is only written.
struct SinkFile(RefCell<Sink>);

impl HostFile for SinkFile {
    fn poll_read(&self, _cx: &mut Context<'_>, _buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other("Bad file descriptor")))
    }

    fn poll_write(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready((self.0.borrow_mut())(buf).map(|()| buf.len()))
    }

    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::Fifo)
    }
}

/// Runs `command` as `psh -c SCRIPT` would, with the null device as stdin, on a new engine
/// thread. `ended` is called on that thread with the status once the shell and what it
/// held are gone, or with the panic that ended the thread.
pub fn start(
    command: Command,
    ended: impl FnOnce(std::thread::Result<i32>) + Send + 'static,
) -> io::Result<Running> {
    let stdin = std::fs::File::open("/dev/null")?;
    let running = Running(Arc::default());
    let shared = running.clone();
    engine_thread(move || {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let host = UnixHost::new(UnixConfig {
                pid: Some(NO_PID),
                ..UnixConfig::default()
            });
            let killer = host.killer();
            let mut cfg = ShellConfig::new(Rc::new(host));
            cfg.env = command.env;
            (cfg.utilities, cfg.nested_shells) = crate::settings(&mut cfg.env);
            cfg.cwd = command.cwd;
            cfg.stdin = Some(Rc::new(UnixFile::new(stdin)));
            cfg.stdout = Some(Rc::new(SinkFile(RefCell::new(command.stdout))));
            cfg.stderr = Some(Rc::new(SinkFile(RefCell::new(command.stderr))));
            let mut shell = Shell::new(cfg);
            let control = shell.control();
            shell.start(Script {
                source: command.script,
                arg0: b"psh".to_vec(),
                args: Vec::new(),
                dash_c: true,
                options: Vec::new(),
                stdin: false,
            });
            {
                let mut state = shared.state();
                if state.stop {
                    control.cancel();
                } else if state.interrupt {
                    control.signal(SIGINT);
                }
                state.shell = Some((control, killer));
            }
            let status = drive(&mut shell);
            shared.state().shell = None;
            status
        }));
        ended(run);
    })?;
    Ok(running)
}
