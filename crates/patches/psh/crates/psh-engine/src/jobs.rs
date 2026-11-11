//! Background jobs (`cmd &`): the table behind `$!`, `wait`, `kill`, `jobs` and `disown`
//! (D1900).
//!
//! A job is a detached vproc task. A job that is one simple command (`cmd args &`) is
//! its program, as in bash, which runs such a job as the program's own process: `$!` is
//! the external's OS pid, so `kill $!`, `wait $!` and a pid written to a file mean the
//! program. Any other job (`{ …; } &`, a pipeline, a builtin) is a subshell in bash; its
//! `$!` here is a pid of its own above every real one, and a signal that ends processes
//! ends the job's task at once (its externals run on, as a killed subshell's children
//! do). Each vproc has its own table (a subshell starts with an empty one: it cannot
//! wait for its parent's jobs); the vprocs of a job know their job, so the externals
//! they start are the job's.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::task::{Poll, Waker};

use crate::host::{ExtProc, Signal};

/// A running external of a job, shared between its wait (`run_external`) and the
/// job (`kill`). Empty once the external is gone.
pub(crate) type ProcCell = RefCell<Option<Box<dyn ExtProc>>>;

/// Pids handed out to jobs that start no external: above Linux's highest pid
/// (`pid_max` ≤ 2^22) and macOS's (99999), so they never name a real process.
const VIRTUAL_PID_BASE: u32 = 1 << 22;

pub(crate) struct Job {
    /// `%n`.
    pub id: u32,
    /// The command as `jobs` shows it.
    pub text: String,
    pid: Cell<Option<u32>>,
    virt: u32,
    status: Cell<Option<i32>>,
    waiters: RefCell<Vec<Waker>>,
    procs: RefCell<Vec<Weak<ProcCell>>>,
    /// A signal delivered to the job's vprocs: they end at their next command.
    killed: Cell<Option<Signal>>,
    /// `disown`: its externals leave the shell's run.
    pub disowned: Cell<bool>,
    /// The signal that ended the job's last external, if one did.
    last_signal: Cell<Option<Signal>>,
    /// The job is one simple command: its external is the job's process.
    direct: bool,
    /// The job's task (`Sched::drop_task`), while it runs.
    task: Cell<Option<usize>>,
}

impl Job {
    /// `$!`: the first external's pid, or the job's own.
    pub(crate) fn pid(&self) -> u32 {
        self.pid.get().unwrap_or(self.virt)
    }

    /// Whether `$!` is final: a direct job's external started, or any other job.
    pub(crate) fn has_pid(&self) -> bool {
        !self.direct || self.pid.get().is_some()
    }

    pub(crate) fn set_task(&self, id: usize) {
        self.task.set(Some(id));
    }

    /// Whether `pid` names this job: its `$!` or any external it ran.
    fn owns(&self, pid: u32) -> bool {
        self.pid() == pid
            || self.virt == pid
            || self.procs.borrow().iter().any(|p| {
                p.upgrade()
                    .and_then(|c| c.borrow().as_ref().and_then(|p| p.pid()))
                    == Some(pid)
            })
    }

    pub(crate) fn status(&self) -> Option<i32> {
        self.status.get()
    }

    /// The job's vproc ended with `status`.
    pub(crate) fn finish(&self, status: i32) {
        self.task.set(None);
        self.status.set(Some(status));
        for w in self.waiters.borrow_mut().drain(..) {
            w.wake();
        }
    }

    /// Waits until the job is done; its status.
    pub(crate) async fn wait(&self) -> i32 {
        std::future::poll_fn(|cx| match self.status.get() {
            Some(s) => Poll::Ready(s),
            None => {
                let mut ws = self.waiters.borrow_mut();
                if !ws.iter().any(|w| w.will_wake(cx.waker())) {
                    ws.push(cx.waker().clone());
                }
                Poll::Pending
            }
        })
        .await
    }

    /// An external of the job started: the first one's pid becomes `$!`.
    pub(crate) fn started(&self, proc: &Rc<ProcCell>) {
        let pid = proc.borrow().as_ref().and_then(|p| p.pid());
        if self.direct
            && self.pid.get().is_none()
            && let Some(pid) = pid
        {
            self.pid.set(Some(pid));
        }
        let mut procs = self.procs.borrow_mut();
        procs.retain(|p| p.strong_count() > 0);
        procs.push(Rc::downgrade(proc));
    }

    /// Sends `sig` to the job. A direct job: to its program (its group). Any other job
    /// is a subshell that ignores SIGINT and SIGQUIT: a signal that ends processes ends
    /// its task now, with status 128 + sig, and its externals run on (released), as a
    /// killed subshell's children would.
    pub(crate) fn signal(&self, sig: Signal, sched: &crate::sched::Sched) {
        if self.status.get().is_some() {
            return;
        }
        let procs: Vec<Rc<ProcCell>> = self
            .procs
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|p| p.borrow().is_some())
            .collect();
        if self.direct && !procs.is_empty() {
            for p in procs {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.signal(sig);
                }
            }
            return;
        }
        let ignored = sig == crate::host::SIGINT || sig == crate::host::SIGQUIT;
        if ignored || crate::signals::harmless(sig) || crate::signals::stops(sig) {
            return;
        }
        self.killed.set(Some(sig));
        // Dropping the task drops the job's waits; their externals are released.
        if let Some(id) = self.task.take()
            && sched.drop_task(id)
        {
            self.finish(128 + sig);
        }
        // Otherwise (it is the task running now) it ends at its next command.
    }

    /// Whether the job's externals are released rather than abandoned when its task is
    /// dropped: it was killed (a killed subshell's children run on) or disowned.
    pub(crate) fn releases(&self) -> bool {
        self.killed.get().is_some() || self.disowned.get()
    }

    /// An external of the job ended (`signal`: by that signal).
    pub(crate) fn ended(&self, signal: Option<Signal>) {
        self.last_signal.set(signal);
    }

    /// The signal that ended the job, when its status says a signal did and its last
    /// external died of it: what `wait` reports.
    pub(crate) fn died_of(&self) -> Option<Signal> {
        let sig = self.killed.get().or(self.last_signal.get())?;
        (self.status.get() == Some(128 + sig)).then_some(sig)
    }

    /// `disown`: hidden from `jobs` and `wait`, and its externals leave the shell's run
    /// (when their wait ends; later ones start detached).
    pub(crate) fn disown(&self) {
        self.disowned.set(true);
    }

    /// The signal that ends the job's vprocs, if one was sent.
    pub(crate) fn killed(&self) -> Option<Signal> {
        self.killed.get()
    }
}

/// Gives a job that has not started an external yet a chance to (`$!` is about to be
/// read): the engine runs other ready tasks, a bounded number of rounds, until the job
/// has a pid, ends, or nothing else can run (it waits for something: it keeps its own
/// pid).
pub(crate) async fn settle(sched: &crate::sched::Sched, job: &Job) {
    let mut rounds = 0;
    std::future::poll_fn(|cx| {
        if job.has_pid() || job.status().is_some() || rounds >= 64 || !sched.has_ready() {
            Poll::Ready(())
        } else {
            rounds += 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}

/// A vproc's jobs, oldest first.
#[derive(Default)]
pub(crate) struct JobTable {
    jobs: Vec<Rc<Job>>,
    /// The parent's jobs, which `jobs` in a pipeline stage still lists (bash does; a
    /// `( … )` subshell lists none). Nothing else sees them.
    shown: Vec<Rc<Job>>,
    /// Finished jobs already reported, newest last: `wait PID` still answers their
    /// status, as bash remembers its last background pids.
    reaped: Vec<Rc<Job>>,
}

impl JobTable {
    /// A pipeline stage's table: empty, but `jobs` lists the shell's.
    pub(crate) fn for_stage(&self) -> JobTable {
        let mut shown = self.shown.clone();
        shown.extend(self.jobs.iter().cloned());
        JobTable {
            jobs: Vec::new(),
            shown,
            reaped: Vec::new(),
        }
    }

    /// What `jobs` lists: the inherited view first, then the own jobs.
    pub(crate) fn listed(&self) -> Vec<Rc<Job>> {
        let mut all = self.shown.clone();
        all.extend(self.jobs.iter().cloned());
        all
    }

    pub(crate) fn add(&mut self, text: String, virt: u32, direct: bool) -> Rc<Job> {
        let id = self.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
        let job = Rc::new(Job {
            id,
            text,
            pid: Cell::new(None),
            virt: VIRTUAL_PID_BASE + virt,
            status: Cell::new(None),
            waiters: RefCell::new(Vec::new()),
            procs: RefCell::new(Vec::new()),
            killed: Cell::new(None),
            disowned: Cell::new(false),
            last_signal: Cell::new(None),
            direct,
            task: Cell::new(None),
        });
        self.jobs.push(job.clone());
        job
    }

    pub(crate) fn all(&self) -> Vec<Rc<Job>> {
        self.jobs.clone()
    }

    /// A finished job was reported: it leaves the table, and only `wait PID` finds it.
    pub(crate) fn remove(&mut self, job: &Rc<Job>) {
        let before = self.jobs.len();
        self.jobs.retain(|j| !Rc::ptr_eq(j, job));
        if self.jobs.len() < before {
            self.reaped.push(job.clone());
            if self.reaped.len() > 64 {
                self.reaped.remove(0);
            }
        }
    }

    /// The job `wait PID` means: a live one, or one already reported.
    pub(crate) fn waitable(&self, pid: u32) -> Option<Rc<Job>> {
        self.by_pid(pid)
            .or_else(|| self.reaped.iter().rev().find(|j| j.owns(pid)).cloned())
    }

    pub(crate) fn by_pid(&self, pid: u32) -> Option<Rc<Job>> {
        self.jobs.iter().rev().find(|j| j.owns(pid)).cloned()
    }

    /// The current job (`%+`, `%%`): the newest; the previous (`%-`): the one before.
    pub(crate) fn current(&self) -> Option<Rc<Job>> {
        self.jobs.last().cloned()
    }

    pub(crate) fn previous(&self) -> Option<Rc<Job>> {
        let n = self.jobs.len();
        if n >= 2 {
            Some(self.jobs[n - 2].clone())
        } else {
            None
        }
    }

    /// The `+` or `-` mark `jobs` shows for `job`.
    pub(crate) fn mark(&self, job: &Rc<Job>) -> char {
        let all = self.listed();
        let n = all.len();
        if n >= 1 && Rc::ptr_eq(&all[n - 1], job) {
            '+'
        } else if n >= 2 && Rc::ptr_eq(&all[n - 2], job) {
            '-'
        } else {
            ' '
        }
    }

    /// A job spec without its `%`: `n`, `%`/`+`, `-`, `name` (prefix of the command),
    /// `?text` (anywhere in it). `Err` is bash's message for a bad spec.
    pub(crate) fn find(&self, spec: &str) -> Result<Rc<Job>, String> {
        let none = || format!("%{spec}: no such job");
        match spec {
            "" | "%" | "+" => self.current().ok_or_else(none),
            "-" => self.previous().or_else(|| self.current()).ok_or_else(none),
            _ if spec.bytes().all(|b| b.is_ascii_digit()) => {
                let n: u32 = spec.parse().map_err(|_| none())?;
                self.jobs
                    .iter()
                    .find(|j| j.id == n)
                    .cloned()
                    .ok_or_else(none)
            }
            _ => {
                let (anywhere, text) = match spec.strip_prefix('?') {
                    Some(t) => (true, t),
                    None => (false, spec),
                };
                let hits: Vec<&Rc<Job>> = self
                    .jobs
                    .iter()
                    .filter(|j| {
                        if anywhere {
                            j.text.contains(text)
                        } else {
                            j.text.starts_with(text)
                        }
                    })
                    .collect();
                match hits.as_slice() {
                    [] => Err(none()),
                    [one] => Ok((*one).clone()),
                    _ => Err(format!("%{spec}: ambiguous job spec")),
                }
            }
        }
    }
}
