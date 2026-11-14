//! `bash` and `sh` named in a command run in psh (D1410): a new shell, started afresh
//! as a new bash process would be (D1300), with psh's own command line
//! (`invocation::parse_args`). It sits in the lookup after functions and builtins and
//! before the utilities, so wrappers (`env`, `xargs`, `find -exec`, `timeout`) reach it,
//! and reports like a utility (`type`, `command -v`, `which`).
//!
//! It runs as a virtual process (D1411): a task of its own behind an [`ExtProc`],
//! started where `run_external_with` starts programs, so jobs, `$!`, `kill`, `wait` and
//! abandoning (a cancelled `timeout`) treat it as they treat a program. It has a `$$`
//! and a pending-signal mask of its own: a signal sent to it runs its trap or ends it.
//!
//! A path (`/bin/bash`, `./bash`), a hashed name and `command -p bash` run the program,
//! except the four Unix shell paths on Windows, where no such files exist. Options psh
//! does not implement run the real program from `PATH`, as utilities fall through
//! (D1082).

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

use super::{LocalBoxFuture, Mode, UtilCtx, UtilEntry};
use crate::Input;
use crate::builtins::trap::{Trap, TrapKey};
use crate::fd::FdObj;
use crate::host::{ExitStatus, ExtProc, FileKind, OpenOptions, Signal, Spawned, error_text};
use crate::invocation::{Invocation, parse_args};
use crate::sched::Join;
use crate::state::{Core, NestedSigs, Sigs, Unwind, Vp};

/// Not in any registry: the engine looks it up itself (`super::lookup`), and
/// `super::run_with` starts it as a virtual process, never through `run`.
pub(crate) static ENTRY: UtilEntry = UtilEntry {
    names: &["bash", "sh"],
    mode: Mode::Always,
    destructive: false,
    run,
};

fn run<'a>(_ctx: &'a mut UtilCtx<'_>, _argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
    Box::pin(async { 2 })
}

/// On Windows these mean psh, as they do in shebangs there.
const WINDOWS_PATHS: [&[u8]; 4] = [b"/bin/sh", b"/bin/bash", b"/usr/bin/sh", b"/usr/bin/bash"];

/// How long an abandoned shell has after its TERM before its task is dropped (KILL),
/// as the OS hosts give a program.
const GRACE: Duration = Duration::from_secs(2);

/// Whether `name` runs as a shell in psh here.
pub(crate) fn is_shell(vp: &Vp, name: &[u8]) -> bool {
    vp.core.nested_shells
        && (matches!(name, b"bash" | b"sh") || (cfg!(windows) && WINDOWS_PATHS.contains(&name)))
}

/// What a nested shell takes from the shell that starts it.
struct Caller {
    /// Its `$$`: the nested shell's `$PPID`.
    shell_pid: Option<u32>,
    /// Its `$BASHPID`: `exec bash` replaces that process, keeping its id as `$$`.
    bashpid: Option<u32>,
    /// Its `$PPID`, for `exec bash`.
    ppid: Option<Vec<u8>>,
    in_place: bool,
}

/// Starts `argv` (`bash …`) as a virtual process: a subshell of `vp` with the prefix
/// assignments exported, then (unless it falls through) a shell started afresh, as its
/// own task. `ignore`: the signals it starts with ignored (`trap '' SIG` passes on,
/// D1284). `in_place`: run by `exec`, it keeps the caller's `$$` and `SHLVL`.
pub(crate) async fn start(
    vp: &mut Vp,
    argv: Vec<Vec<u8>>,
    assigns: &[(String, Vec<u8>)],
    ignore: Vec<Signal>,
    in_place: bool,
    process: bool,
) -> Result<Spawned, Unwind> {
    let mut sub = crate::exec::enter(vp).await?;
    for (name, value) in assigns {
        let var = sub.vars.entry(name);
        var.set(value.clone());
        var.exported = true;
        sub.assigned_dynamic(name);
    }
    // Its own process: a job it runs in is the caller's business (a killed subshell's
    // children run on).
    sub.job = None;
    sub.exec_tail = None;
    sub.exec_now = false;
    sub.top = true;
    let sigs = Rc::new(NestedSigs::default());
    let own = (in_place || process) && vp.top && matches!(vp.sigs, Sigs::Shell);
    sub.sigs = match own {
        true => Sigs::Shell,
        false => Sigs::Nested(sigs.clone()),
    };
    let caller = Caller {
        shell_pid: vp.shell_pid,
        bashpid: vp.bashpid,
        ppid: vp.var("PPID").map(<[u8]>::to_vec),
        in_place,
    };
    // Its job id for `$!`, `kill` and `wait`; never its `$$` (D1413).
    let pid = vp.core.next_job_id();
    let watch = sigs.clone();
    let core = vp.core.clone();
    let (id, join) = vp.core.sched.spawn_with_id(async move {
        let status = shell(sub, argv, ignore, caller).await;
        let died = match own {
            true => core.died_of.get(),
            false => watch.died_of.get(),
        };
        match died {
            Some(sig) if status == 128 + sig => ExitStatus::Signaled(sig),
            _ => ExitStatus::Exited(status),
        }
    });
    let proc = NestedProc {
        core: vp.core.clone(),
        id,
        join: Some(join),
        sigs,
        pid,
        status: None,
        released: false,
    };
    Ok(Spawned {
        proc: Box::new(proc),
        pipes: Vec::new(),
    })
}

/// The shell's task: its command line, then its script.
async fn shell(mut sub: Vp, argv: Vec<Vec<u8>>, ignore: Vec<Signal>, caller: Caller) -> i32 {
    let (input, arg0, args, options, dash_c) = match parse_args(&argv[1..]) {
        Ok(Invocation::Command {
            source,
            arg0,
            args,
            options,
        }) => (
            Input::Text(source),
            arg0.unwrap_or_else(|| argv[0].clone()),
            args,
            options,
            true,
        ),
        Ok(Invocation::File {
            path,
            args,
            options,
        }) => match read_script(&sub, &argv[0], &path).await {
            Ok(source) => (Input::Text(source), path, args, options, false),
            Err(status) => return status,
        },
        Ok(Invocation::Version) => {
            let text = format!("psh {}\n", env!("CARGO_PKG_VERSION"));
            return match sub.fd(1) {
                Some(f) => i32::from(f.write_all(text.as_bytes()).await.is_err()),
                None => 1,
            };
        }
        // Read one command at a time as it runs (D1414): nothing to scan ahead.
        Ok(Invocation::Stdin { args, options }) => {
            (Input::Stdin, argv[0].clone(), args, options, false)
        }
        Ok(Invocation::Help) => return fall_through(&mut sub, argv, "--help: not supported").await,
        Err(msg) => return fall_through(&mut sub, argv, &msg).await,
    };
    // No process id to give (D1413): a script that reads one runs under the real
    // program, if there is one; else reading it fails.
    if let Input::Text(source) = &input
        && crate::pidscan::uses_pid(source)
        && let Some(status) = real_program(&mut sub, &argv).await
    {
        return status;
    }
    let shlvl = sub.var("SHLVL").map(<[u8]>::to_vec);
    crate::exec::start_afresh(&mut sub);
    if caller.in_place {
        // `exec bash`: the process it replaces goes on, and its id is the new `$$`;
        // bash lowers `SHLVL` before it execs, the new shell raises it again.
        // From a subshell, the process it replaces is the shell's child.
        let ppid = match caller.bashpid {
            Some(_) => caller.ppid,
            None => caller.shell_pid.map(|p| p.to_string().into_bytes()),
        };
        sub.shell_pid = caller.bashpid;
        sub.bashpid = caller.bashpid;
        set_ppid(&mut sub, ppid);
        if let (Some(value), Some(var)) = (shlvl, sub.vars.get_mut("SHLVL")) {
            var.value = crate::vars::Value::Str(value);
        }
    } else {
        sub.shell_pid = None;
        sub.bashpid = None;
        set_ppid(
            &mut sub,
            caller.shell_pid.map(|p| p.to_string().into_bytes()),
        );
    }
    // Signals ignored when a shell starts stay ignored (bash).
    if !ignore.is_empty() {
        let traps = Rc::make_mut(&mut sub.traps);
        for sig in ignore {
            traps.map.insert(
                TrapKey::Signal(sig),
                Trap {
                    action: Rc::from(&b""[..]),
                    active: true,
                },
            );
        }
    }
    sub.arg0 = arg0;
    sub.script_args = args.clone();
    sub.args = args;
    sub.opts.dash_c = dash_c;
    crate::builtins::apply_invocation_options(&mut sub, &options);
    let (status, _) = Box::pin(crate::run_script(sub, input)).await;
    status
}

/// `$PPID`, a readonly integer; unset when there is no such process.
fn set_ppid(sub: &mut Vp, value: Option<Vec<u8>>) {
    match value {
        Some(v) => {
            let ppid = sub.vars.entry("PPID");
            ppid.set(v);
            ppid.integer = true;
            ppid.readonly = true;
        }
        None => {
            sub.vars.remove("PPID");
        }
    }
}

/// An option psh lacks: the real program from `PATH`
/// runs, else the shell fails with status 2 naming psh.
async fn fall_through(sub: &mut Vp, argv: Vec<Vec<u8>>, why: &str) -> i32 {
    if let Some(status) = real_program(sub, &argv).await {
        return status;
    }
    let name = String::from_utf8_lossy(&argv[0]).into_owned();
    let text = format!("{name}: {why} (psh built-in {name})\n");
    sub.error_raw(text.as_bytes()).await;
    2
}

/// Runs the first program named `argv[0]` in `PATH` that is not psh itself (D1413):
/// its status, or `None` when there is none. Searched in a sandbox too: its host may
/// have a `bash`, and a miss is recorded. `$0` stays the name as written.
async fn real_program(sub: &mut Vp, argv: &[Vec<u8>]) -> Option<i32> {
    let name = &argv[0];
    let program = if name.contains(&b'/') {
        // `/bin/bash` on Windows, which does not exist there.
        crate::builtins::hash::search(sub, name, None).await?
    } else {
        let path = sub
            .var("PATH")
            .map(<[u8]>::to_vec)
            .unwrap_or_else(|| crate::exec::DEFAULT_PATH.to_vec());
        let own = match sub.core.own_program.clone() {
            Some(p) => Some(sub.host().canonicalize(&p).await.unwrap_or(p)),
            None => None,
        };
        let mut found = None;
        for dir in sub.host().path_entries(&path) {
            let Some(p) = crate::builtins::hash::search(sub, name, Some(&dir)).await else {
                continue;
            };
            let p = sub.abs_path(&p);
            if let Some(own) = &own
                && sub
                    .host()
                    .canonicalize(&p)
                    .await
                    .unwrap_or_else(|_| p.clone())
                    == *own
            {
                continue;
            }
            found = Some(p);
            break;
        }
        found?
    };
    let mut argv = argv.to_vec();
    argv[0] = program;
    let opts = crate::exec::ExtOpts {
        arg0: Some(name.clone()),
        real: true,
        ..Default::default()
    };
    let r = crate::exec::run_external_with(sub, argv, Vec::new(), opts).await;
    // A program a signal ended: so did this shell, for the caller's report (D1303).
    if let (Some((sig, pid)), Sigs::Nested(n)) = (sub.core.fg_signal.take(), &sub.sigs) {
        n.died_of.set(Some(sig));
        n.real_pid.set(Some(pid));
    }
    let last = sub.status;
    Some(r.unwrap_or_else(|u| u.status(last)))
}

/// The script `bash FILE` runs: `FILE` from the cwd, else (no slash) from a directory
/// of `PATH`, as bash looks for it (it need not be executable). Errors as bash's: 127
/// for a missing file, 126 for one that cannot be read, a directory or a binary file.
async fn read_script(sub: &Vp, name: &[u8], path: &[u8]) -> Result<Vec<u8>, i32> {
    let shown = String::from_utf8_lossy(path).into_owned();
    let host = sub.host();
    let mut target = sub.abs_path(path);
    if !path.contains(&b'/')
        && host.stat(&target, true).await.is_err()
        && let Some(found) = search_path(sub, path).await
    {
        target = found;
    }
    let mut source = Vec::new();
    let read = match crate::exec::dev_fd(path) {
        // The shell's own descriptors (`bash /dev/stdin`, D1055).
        Some(n) => match sub.fd(n) {
            Some(f) => f.read_to_end(&mut source).await,
            None => Err(crate::fd::bad_fd()),
        },
        None => match host.open(&target, OpenOptions::read()).await {
            Ok(f) => FdObj::File(f).read_to_end(&mut source).await,
            Err(e) => Err(e),
        },
    };
    if let Err(e) = read {
        if matches!(host.stat(&target, true).await, Ok(m) if m.kind == FileKind::Dir) {
            return Err(refused(sub, &shown, "Is a directory").await);
        }
        let name = String::from_utf8_lossy(name);
        let text = format!("{name}: {shown}: {}\n", error_text(&e));
        sub.error_raw(text.as_bytes()).await;
        return Err(if e.kind() == io::ErrorKind::NotFound {
            127
        } else {
            126
        });
    }
    // bash's test, as for a file it runs itself (D1300): a NUL in the first line of the
    // first 80 bytes.
    let head = &source[..source.len().min(80)];
    let first_line = head.split(|&b| b == b'\n').next().unwrap_or(head);
    if first_line.contains(&0) {
        return Err(refused(sub, &shown, "cannot execute binary file").await);
    }
    Ok(source)
}

/// bash names the script twice for these: its `$0` is the file already. Status 126.
async fn refused(sub: &Vp, shown: &str, what: &str) -> i32 {
    sub.error_raw(format!("{shown}: {shown}: {what}\n").as_bytes())
        .await;
    126
}

/// The first regular file named `name` in a directory of the command's `PATH`.
async fn search_path(sub: &Vp, name: &[u8]) -> Option<Vec<u8>> {
    let path = sub
        .var("PATH")
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| crate::exec::DEFAULT_PATH.to_vec());
    for dir in sub.host().path_entries(&path) {
        let mut p = dir;
        if !p.is_empty() {
            p.push(b'/');
        }
        p.extend_from_slice(name);
        let p = sub.abs_path(&p);
        if matches!(sub.host().stat(&p, true).await, Ok(m) if m.kind == FileKind::File) {
            return Some(p);
        }
    }
    None
}

/// A running shell psh started for `bash`/`sh`, seen as a process (D1411).
struct NestedProc {
    core: Rc<Core>,
    /// Its task; the id is its own while `join` is not done.
    id: usize,
    join: Option<Join<ExitStatus>>,
    sigs: Rc<NestedSigs>,
    pid: u32,
    status: Option<ExitStatus>,
    released: bool,
}

impl ExtProc for NestedProc {
    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<ExitStatus> {
        if let Some(s) = self.status {
            return Poll::Ready(s);
        }
        let Some(join) = &mut self.join else {
            return Poll::Pending;
        };
        match Pin::new(join).poll(cx) {
            Poll::Ready(s) => {
                self.status = Some(s);
                Poll::Ready(s)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Signal> {
        self.sigs.poll_stop(cx)
    }

    fn signal(&mut self, sig: Signal) {
        if self.status.is_none() {
            if sig == crate::host::SIGCONT {
                self.sigs.cont.set(true);
            }
            self.sigs.raise(sig);
        }
    }

    fn release(mut self: Box<Self>) {
        self.released = true;
    }

    fn pid(&self) -> Option<u32> {
        Some(self.sigs.real_pid.get().unwrap_or(self.pid))
    }
}

impl Drop for NestedProc {
    /// Abandoned (a cancelled wait, `timeout`): TERM to it and, as to a program's
    /// process group, to what it waits for; its task is dropped after the grace if it
    /// still runs, as a program gets TERM, then KILL. Not while every task
    /// is being dropped (a cancel): its task is gone then, and nothing may spawn.
    fn drop(&mut self) {
        let Some(join) = self.join.take() else {
            return;
        };
        if self.released || self.status.is_some() || join.is_done() {
            return;
        }
        if !self.core.sched.has_task(self.id) {
            return;
        }
        self.sigs.group.set(true);
        self.sigs.raise(crate::host::SIGTERM);
        let sched = self.core.sched.clone();
        let mut sleep = self.core.host.sleep(GRACE);
        let id = self.id;
        let mut join = join;
        self.core.sched.spawn(async move {
            let ended = std::future::poll_fn(|cx| {
                if Pin::new(&mut join).poll(cx).is_ready() {
                    return Poll::Ready(true);
                }
                sleep.as_mut().poll(cx).map(|()| false)
            })
            .await;
            if !ended {
                sched.drop_task(id);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::testhost::TestHost;
    use crate::tests::{check, run, run_setup};
    use crate::util::{LocalBoxFuture, Mode, SpawnEnv, UtilCtx, UtilEntry, UtilPolicy};
    use crate::{Script, Shell, ShellConfig, Step};

    /// Runs `src` on a fresh test host with `cfg` changed by `adjust`; status, stdout
    /// and stderr. Checks that nothing is left behind.
    fn run_cfg(src: &str, adjust: impl Fn(&mut ShellConfig)) -> (i32, String, String) {
        let host = Rc::new(TestHost::new());
        let mut cfg = host.config();
        adjust(&mut cfg);
        let mut shell = Shell::new(cfg);
        shell.start(Script::command(src));
        let status = loop {
            match shell.step(7) {
                Step::Done(s) => break s,
                Step::Yield => {}
                other => panic!("{other:?} running {src:?}"),
            }
        };
        assert_eq!(shell.task_count(), 0, "tasks left after {src:?}");
        assert_eq!(host.live_procs(), 0, "processes left after {src:?}");
        (
            status,
            String::from_utf8_lossy(&host.stdout()).into_owned(),
            String::from_utf8_lossy(&host.stderr()).into_owned(),
        )
    }

    #[test]
    fn runs_without_a_policy_or_path() {
        // The test host has no `bash` program and the engine no utilities.
        check("bash -c 'echo \"$0 $1\"' a b", "a b\n");
        check("sh -c 'echo $0'", "sh\n");
        check("PATH=/nonexistent bash -c 'echo ok'", "ok\n");
        check("bash -c 'exit 3'; echo $?", "3\n");
        check("bash -c 'bash -c \"echo deep\"'", "deep\n");
    }

    #[test]
    fn starts_afresh() {
        check(
            "x=1; export y=2; f() { :; }; set -e; trap 'echo trap' EXIT; \
             bash -c 'echo \"${x-unset} $y $-\"; type -t f || echo no-f; trap'",
            "unset 2 hBc\nno-f\ntrap\n",
        );
        check(
            "bash -c 'echo \"$SHLVL $BASH_SUBSHELL $PPID [$IFS]\"'",
            "2 0 4242 [ \t\n]\n",
        );
        check("export SHELLOPTS; set -u; bash -c 'echo $-'", "huBc\n");
        // `exec`: the same process goes on.
        check("(exec bash -c 'echo \"$SHLVL $PPID\"')", "1 4242\n");
        check("exec -a me bash -c 'echo $0'", "me\n");
    }

    /// Without a real `bash`, `$$` and `BASHPID` in a nested shell end it as `${x:?}`
    /// would; `BASHPID` in a subshell too; the shell's own stay (D1413).
    #[test]
    fn no_process_id() {
        let o = run("bash -c 'echo $$; echo no'; echo $?; sh -c 'echo ${#BASHPID}'; echo $?");
        assert_eq!(o.stdout, "127\n127\n");
        assert_eq!(
            o.stderr,
            "bash: line 1: $$: this shell runs inside psh and has no process id\n\
             sh: line 1: BASHPID: this shell runs inside psh and has no process id\n"
        );
        // Only when it is read: at run time, past the scan.
        check(
            "bash -c 'echo ok; x=$; eval \"echo \\$$x\"; echo no'",
            "ok\n",
        );
        check("bash -c 'echo $PPID'", "4242\n");
        check("exec bash -c 'echo $$ $BASHPID'", "4242 4242\n");
        check("echo $$ $BASHPID; (echo $$)", "4242 4242\n4242\n");
        let o = run("(echo $BASHPID; echo no); echo $?; echo $(( BASHPID ))");
        assert_eq!(o.stdout, "1\n4242\n");
        assert_eq!(
            o.stderr,
            "psh: line 1: BASHPID: this subshell runs inside psh and has no process id\n"
        );
    }

    /// A script that reads `$$` or `BASHPID` runs under the real program (not psh
    /// itself); one that does not stays in psh.
    #[test]
    fn pid_uses_run_the_real_program() {
        let real = |h: &TestHost| {
            h.add_command("bash", |p| {
                Box::pin(async move {
                    let mut line = b"real".to_vec();
                    for a in &p.argv {
                        line.push(b' ');
                        line.extend_from_slice(a);
                    }
                    line.push(b'\n');
                    let _ = p.write(&line).await;
                    0
                })
            });
        };
        let o = run_setup(
            "bash -c 'echo $$'; bash -c 'bash -c \"echo \\$BASHPID\"'; bash -c 'echo in-psh'",
            real,
        );
        assert_eq!(
            o.stdout,
            "real bash -c echo $$\nreal bash -c bash -c \"echo \\$BASHPID\"\nin-psh\n"
        );
        let o = run_setup("printf 'echo $$\\n' > s.sh; bash s.sh", real);
        assert_eq!(o.stdout, "real bash s.sh\n");
        // psh itself is never the real program.
        let host = Rc::new(TestHost::new());
        real(&host);
        let mut cfg = host.config();
        cfg.own_program = Some(b"/bin/bash".to_vec());
        let mut shell = Shell::new(cfg);
        shell.start(Script::command("bash -c 'echo $$'; echo $?"));
        let status = loop {
            if let Step::Done(s) = shell.step(7) {
                break s;
            }
        };
        assert_eq!(status, 0);
        assert_eq!(host.stdout(), b"127\n");
    }

    #[test]
    fn reported_like_a_utility() {
        check(
            "type bash; type -t sh; command -v bash; command -V sh",
            "bash is a psh utility\nbuiltin\nbash\nsh is a psh utility\n",
        );
    }

    #[test]
    fn paths_and_hashed_names_run_the_program() {
        let o = run("/bin/bash -c 'echo psh'; echo $?");
        if cfg!(windows) {
            assert_eq!(o.stdout, "psh\n0\n");
        } else {
            assert_eq!(o.stdout, "127\n", "{}", o.stderr);
        }
        check("command -p bash -c x 2>/dev/null; echo $?", "127\n");
        check("hash -p /bin/true bash; bash -c 'echo psh'; echo $?", "0\n");
    }

    #[test]
    fn off_switch() {
        let (st, out, err) = run_cfg("bash -c 'echo psh'; type -t sh", |c| {
            c.nested_shells = false
        });
        assert_eq!((st, out.as_str()), (1, ""), "{err}");
        assert!(err.contains("bash: command not found"), "{err}");
    }

    #[test]
    fn unsupported_options_fall_through() {
        let o = run("bash -i -c 'echo x'; echo $?");
        assert_eq!(o.stdout, "2\n");
        assert_eq!(
            o.stderr,
            "bash: -i: interactive shells are not supported (psh built-in bash)\n"
        );
        // A script on stdin runs in psh (D1414).
        check("echo 'echo x' | sh; echo $?", "x\n0\n");
        // The real program when `PATH` has one.
        let o = run_setup("bash -i -c x; bash --rcfile f; bash -r; echo $?", |h| {
            h.add_command("bash", |p| {
                Box::pin(async move {
                    let mut line = b"real".to_vec();
                    for a in &p.argv[1..] {
                        line.push(b' ');
                        line.extend_from_slice(a);
                    }
                    line.push(b'\n');
                    let _ = p.write(&line).await;
                    5
                })
            });
        });
        assert_eq!(o.stdout, "real -i -c x\nreal --rcfile f\nreal -r\n5\n");
    }

    /// A script on stdin is read one command at a time: its commands read the rest.
    #[test]
    fn scripts_on_stdin() {
        check(
            "printf 'read x\\nhello\\necho got $x\\n' | bash",
            "got hello\n",
        );
        check(
            "bash -s a b <<'EOF'\nread -r l\nline two\necho \"$0 $1 $2 $l\"\ncat <<E2\nbody $1\nE2\nif true\nthen echo multi\nfi\nEOF",
            "bash a b line two\nbody a\nmulti\n",
        );
        let o = run("printf 'echo a\\necho )\\necho b\\n' | sh; echo $?");
        assert_eq!(o.stdout, "a\n2\n");
        assert_eq!(
            o.stderr,
            "sh: line 2: syntax error near unexpected token `)'\nsh: line 2: `echo )'\n"
        );
        check("printf 'echo \\\\\\n  joined\\n' | bash", "joined\n");
    }

    #[test]
    fn files_and_errors() {
        let o = run_setup(
            "bash s.sh a; bash -n bad.sh; echo $?; bash nope.sh; echo $?; bash -n s.sh; echo $?",
            |h| {
                h.fs.write_file("/tmp/s.sh", b"echo \"$0 $1\"\n");
                h.fs.write_file("/tmp/bad.sh", b"if true; then\n");
            },
        );
        assert_eq!(o.stdout, "s.sh a\n2\n127\n0\n");
        assert_eq!(
            o.stderr,
            "bad.sh: line 2: syntax error: unexpected end of file from `if' command on line 1\n\
             bash: nope.sh: No such file or directory\n"
        );
    }

    #[test]
    fn signals_reach_it() {
        // A job's `$!` is the nested shell; its trap runs.
        check(
            "bash -c 'trap \"echo got; exit 3\" TERM; echo r > r; while :; do :; done' & \
             while [ ! -e r ]; do :; done; kill $!; wait $!; echo $?",
            "got\n3\n",
        );
        // A signal it traps leaves it running.
        check(
            "bash -c 'trap \"echo usr1\" USR1; echo r > r2; while [ ! -e go ]; do :; done' & \
             while [ ! -e r2 ]; do :; done; kill -USR1 $!; \
             while [ ! -e go ]; do :> go; done; wait; echo end",
            "usr1\nend\n",
        );
        // `kill 0` from inside reaches its own trap.
        check(
            "bash -c 'trap \"echo t\" TERM; kill -TERM 0; echo after'",
            "t\nafter\n",
        );
    }

    /// `drop CMD…`: starts the command, gives it a step, drops it (as `timeout` does).
    fn drop_child<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(async move {
            let mut child = ctx.spawn(argv[1..].to_vec(), SpawnEnv::default()).await;
            let mut polls = 0;
            std::future::poll_fn(|cx| {
                polls += 1;
                if polls > 20 {
                    return std::task::Poll::Ready(());
                }
                let _ = child.poll_wait(cx);
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            })
            .await;
            drop(child);
            0
        })
    }

    static DROPPER: &[UtilEntry] = &[UtilEntry {
        names: &["drop"],
        mode: Mode::Always,
        destructive: false,
        run: drop_child,
    }];

    #[test]
    fn abandoned_gets_term_and_ends() {
        let policy = || UtilPolicy {
            entries: DROPPER,
            ..UtilPolicy::default()
        };
        // The test host's clock does not wait: the grace is over at once and the shell
        // is dropped (KILL) before its trap runs; the psh binary's tests see the trap.
        let (st, out, _) = run_cfg(
            "drop bash -c 'trap \"echo term\" TERM; echo started; hang'; echo end",
            |c| c.utilities = policy(),
        );
        assert_eq!((st, out.as_str()), (0, "started\nend\n"));
        let (_, out, _) = run_cfg("drop bash -c 'echo started; hang; echo no'", |c| {
            c.utilities = policy()
        });
        assert_eq!(out, "started\n");
    }

    #[test]
    fn cancel_ends_nested_shells() {
        let host = Rc::new(TestHost::new());
        let mut shell = Shell::new(host.config());
        shell.start(Script::command("bash -c 'bash -c hang & hang'"));
        for _ in 0..50 {
            assert!(matches!(shell.step(7), Step::Yield | Step::Blocked));
        }
        shell.control().cancel();
        let status = loop {
            if let Step::Done(s) = shell.step(7) {
                break s;
            }
        };
        assert_eq!(status, 143);
        assert_eq!(shell.task_count(), 0);
        assert_eq!(host.live_procs(), 0);
    }
}
