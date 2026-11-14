//! `wait`, `kill`, `jobs` and `disown` over the vproc's job table (D1900).

use std::rc::Rc;

use crate::host::{SIGTERM, Signal, error_text};
use crate::jobs::Job;
use crate::signals;
use crate::state::{R, Vp};

use super::{out, s};

/// `%spec` → the job; `Err` is bash's message without the builtin's name.
fn job_spec(vp: &Vp, word: &str) -> Result<Rc<Job>, String> {
    let spec = word.strip_prefix('%').unwrap_or(word);
    vp.jobs.borrow().find(spec)
}

/// `wait` reports a job a signal ended, except INT, PIPE and TERM, as bash does:
/// `NAME: line N: PID DESCRIPTION COMMAND`.
async fn report(vp: &Vp, job: &Job) {
    let Some(sig) = job.died_of() else { return };
    let quiet = [crate::host::SIGINT, crate::host::SIGPIPE, SIGTERM];
    if quiet.contains(&sig) {
        return;
    }
    let desc = signals::describe(sig);
    vp.error(&format!("{:>5} {desc:<27}{}", job.pid(), job.text))
        .await;
}

/// Removes a finished job from the table once `wait` or `jobs` reported it.
fn reap(vp: &mut Vp, job: &Rc<Job>) {
    if job.status().is_some() {
        vp.jobs.borrow_mut().remove(job);
        if let Some(name) = job.coproc.take() {
            let pid = format!("{name}_PID");
            if vp.var(&pid) == Some(job.pid().to_string().as_bytes()) {
                vp.vars.remove(&pid);
                vp.vars.remove(&name);
            }
        }
    }
}

const WAIT_USAGE: &str = "wait: usage: wait [-fn] [-p var] [id ...]\n";

/// `wait [-fn] [-p var] [id ...]`
pub(super) async fn wait(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut next = false;
    let mut var: Option<String> = None;
    let mut i = 1;
    while let Some(a) = args.get(i) {
        let a = s(a);
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            break;
        }
        let mut chars = a[1..].chars();
        while let Some(c) = chars.next() {
            match c {
                'n' => next = true,
                'f' => {}
                'p' => {
                    let rest: String = chars.by_ref().collect();
                    let name = if rest.is_empty() {
                        i += 1;
                        match args.get(i) {
                            Some(v) => s(v).into_owned(),
                            None => {
                                vp.error("wait: -p: option requires an argument").await;
                                vp.error_raw(WAIT_USAGE.as_bytes()).await;
                                return Ok(2);
                            }
                        }
                    } else {
                        rest
                    };
                    var = Some(name);
                }
                _ => {
                    vp.error(&format!("wait: -{c}: invalid option")).await;
                    vp.error_raw(WAIT_USAGE.as_bytes()).await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let ids = &args[i..];
    // The jobs named, in order, and the errors for the others (reported below; `-n`
    // with a job already done returns before them, as bash does).
    let mut targets: Vec<Option<Rc<Job>>> = Vec::new();
    let mut errors: Vec<(String, i32)> = Vec::new();
    for id in ids {
        let w = s(id);
        let found = if w.starts_with('%') {
            job_spec(vp, &w).map_err(|msg| (msg, 127))
        } else if let Ok(pid) = w.parse::<u32>() {
            vp.jobs
                .borrow()
                .waitable(pid)
                .ok_or_else(|| (format!("pid {pid} is not a child of this shell"), 127))
        } else {
            Err((format!("`{w}': not a pid or valid job spec"), 1))
        };
        match found {
            Ok(j) => targets.push(Some(j)),
            Err(e) => {
                targets.push(None);
                errors.push(e);
            }
        }
    }
    if next && let Some(done) = targets.iter().flatten().find(|j| j.status().is_some()) {
        let done = done.clone();
        report(vp, &done).await;
        if let Some(v) = &var {
            vp.set_var(v, done.pid().to_string().into_bytes());
        }
        reap(vp, &done);
        return Ok(done.status().unwrap_or(0));
    }
    let mut status = 0;
    for (msg, st) in errors {
        vp.error(&format!("wait: {msg}")).await;
        status = st;
    }
    if next {
        // The first of the named jobs (or of all) to finish.
        let pool: Vec<Rc<Job>> = if ids.is_empty() {
            vp.jobs
                .borrow()
                .all()
                .into_iter()
                .filter(|j| !j.disowned.get())
                .collect()
        } else {
            targets.iter().flatten().cloned().collect()
        };
        if pool.is_empty() {
            return Ok(127);
        }
        let done = std::future::poll_fn(|cx| {
            for j in &pool {
                if j.status().is_some() {
                    return std::task::Poll::Ready(j.clone());
                }
            }
            for j in &pool {
                let mut w = Box::pin(j.wait());
                let _ = w.as_mut().poll(cx);
            }
            std::task::Poll::Pending
        })
        .await;
        let st = done.status().unwrap_or(0);
        report(vp, &done).await;
        if let Some(v) = &var {
            vp.set_var(v, done.pid().to_string().into_bytes());
        }
        reap(vp, &done);
        return Ok(st);
    }
    if ids.is_empty() {
        // Every job; the status is 0.
        let all = vp.jobs.borrow().all();
        for j in all
            .iter()
            .filter(|j| !j.disowned.get() && j.stopped().is_none())
        {
            if let Err(sig) = wait_job(vp, j).await? {
                return Ok(128 + sig);
            }
            report(vp, j).await;
        }
        for j in &all {
            reap(vp, j);
        }
        return Ok(0);
    }
    for j in targets.into_iter().flatten() {
        if j.stopped().is_some() {
            vp.error(&format!("warning: wait_for_job: job {} is stopped", j.id))
                .await;
        }
        // A trapped or fatal signal ends the wait with 128+n; the trap runs next.
        status = match wait_job(vp, &j).await? {
            Ok(s) => s,
            Err(sig) => return Ok(128 + sig),
        };
        report(vp, &j).await;
        if let Some(v) = &var {
            vp.set_var(v, j.pid().to_string().into_bytes());
        }
        reap(vp, &j);
    }
    Ok(status)
}

async fn wait_job(vp: &mut Vp, job: &Job) -> Result<Result<i32, Signal>, crate::state::Unwind> {
    loop {
        match signals::or_signal(vp, signals::Gives::Trapped, job.wait_change()).await {
            Err(crate::host::SIGCHLD) if !vp.opts.get(crate::options::SetOpt::Posix) => {
                super::trap::take_signals(vp).await?;
            }
            Ok(Err(stop)) => return Ok(Ok(128 + stop)),
            Ok(Ok(s)) => return Ok(Ok(s)),
            Err(sig) => return Ok(Err(sig)),
        }
    }
}

async fn job_control(vp: &mut Vp, name: &str, args: &[Vec<u8>]) -> Result<Vec<Rc<Job>>, i32> {
    if !vp.opts.get(crate::options::SetOpt::Monitor) {
        vp.error(&format!("{name}: no job control")).await;
        return Err(1);
    }
    let mut words = &args[1..];
    if words.first().is_some_and(|w| w == b"--") {
        words = &words[1..];
    } else if let Some(w) = words.first()
        && w.len() > 1
        && w[0] == b'-'
    {
        vp.error(&format!("{name}: -{}: invalid option", char::from(w[1])))
            .await;
        let usage = if name == "fg" {
            "fg: usage: fg [job_spec]\n"
        } else {
            "bg: usage: bg [job_spec ...]\n"
        };
        vp.error_raw(usage.as_bytes()).await;
        return Err(2);
    }
    if words.is_empty() {
        let current = vp.jobs.borrow().current();
        return match current {
            Some(j) => Ok(vec![j]),
            None => {
                vp.error(&format!("{name}: current: no such job")).await;
                Err(1)
            }
        };
    }
    let mut found = Vec::new();
    for w in words {
        match job_spec(vp, &s(w)) {
            Ok(j) => found.push(j),
            Err(msg) => {
                vp.error(&format!("{name}: {msg}")).await;
                return Err(1);
            }
        }
    }
    Ok(found)
}

pub(super) async fn fg(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let job = match job_control(vp, "fg", args).await {
        Ok(jobs) => jobs[0].clone(),
        Err(st) => return Ok(st),
    };
    if job.status().is_some() && job.stopped().is_none() && !job.has_pid() {
        vp.error("fg: job has terminated").await;
        return Ok(1);
    }
    let st = out(vp, "fg", format!("{}\n", job.text).as_bytes()).await?;
    if st != 0 {
        return Ok(st);
    }
    job.resume();
    match signals::or_signal(vp, signals::Gives::Fatal, job.wait_change()).await {
        Ok(Ok(s)) => {
            report(vp, &job).await;
            reap(vp, &job);
            Ok(s)
        }
        Ok(Err(stop)) => Ok(128 + stop),
        Err(sig) => Ok(128 + sig),
    }
}

pub(super) async fn bg(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let jobs = match job_control(vp, "bg", args).await {
        Ok(jobs) => jobs,
        Err(st) => return Ok(st),
    };
    let mut status = 0;
    for job in jobs {
        if job.status().is_some() {
            vp.error("bg: job has terminated").await;
            status = 1;
        } else if job.stopped().is_none() {
            vp.error(&format!("bg: job {} already in background", job.id))
                .await;
        } else {
            job.resume();
            let mark = vp.jobs.borrow().mark(&job);
            let st = out(
                vp,
                "bg",
                format!("[{}]{} {} &\n", job.id, mark, job.text).as_bytes(),
            )
            .await?;
            if st != 0 {
                status = st;
            }
        }
    }
    Ok(status)
}

const KILL_USAGE: &str = "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]\n";

/// `kill [-s sigspec | -n signum | -sigspec] pid | jobspec ...`, `kill -l [sigspec]`
pub(super) async fn kill(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut sig: Option<Signal> = None;
    let mut i = 1;
    let invalid = |w: &str| format!("kill: {w}: invalid signal specification");
    while let Some(a) = args.get(i) {
        let a = s(a).into_owned();
        if a == "--" {
            i += 1;
            break;
        }
        if a == "-l" || a == "-L" {
            let mut rest = &args[i + 1..];
            if rest.first().is_some_and(|w| w == b"--") {
                rest = &rest[1..];
            }
            return list(vp, rest).await;
        }
        let spec = if a == "-s" || a == "-n" {
            i += 1;
            match args.get(i) {
                Some(v) => s(v).into_owned(),
                None => {
                    vp.error(&format!("kill: {a}: option requires an argument"))
                        .await;
                    return Ok(1);
                }
            }
        } else if let Some(rest) = a.strip_prefix("-s").or_else(|| a.strip_prefix("-n"))
            && sig.is_none()
            && signals::parse(rest).is_some()
        {
            rest.to_string()
        } else if a.len() > 1 && a.starts_with('-') && sig.is_none() {
            a[1..].to_string()
        } else {
            // A `-N` after the signal is a process group: the operands begin.
            break;
        };
        match signals::parse(&spec) {
            Some(n) => sig = Some(n),
            None => {
                vp.error(&invalid(&spec)).await;
                return Ok(1);
            }
        }
        i += 1;
    }
    let ops = &args[i..];
    if ops.is_empty() {
        vp.error_raw(KILL_USAGE.as_bytes()).await;
        return Ok(2);
    }
    let sig = sig.unwrap_or(SIGTERM);
    let mut status = 0;
    for op in ops {
        let w = s(op).into_owned();
        if w.starts_with('%') {
            match job_spec(vp, &w) {
                Ok(j) => {
                    crate::jobs::settle(&vp.core.sched, &j).await;
                    j.signal(sig, &vp.core.sched)
                }
                Err(msg) => {
                    vp.error(&format!("kill: {msg}")).await;
                    status = 1;
                }
            }
            continue;
        }
        let Ok(pid) = w.parse::<i32>() else {
            vp.error(&format!("kill: `{w}': not a pid or valid job spec"))
                .await;
            status = 1;
            continue;
        };
        // The shell itself (`$$`, and its group with `0`): the signal goes to the
        // shell's pending mask, so traps run (D1284); with 0 the host signals the
        // rest of the group too, where it may.
        // A shell psh runs for `bash`/`sh` has a `$$` and a mask of its own (D1411).
        let me = vp.shell_pid.filter(|_| vp.core.forked.get().is_none());
        if pid == 0
            || u32::try_from(pid).ok() == me
            || (me.is_some() && pid.checked_neg().and_then(|g| u32::try_from(g).ok()) == me)
        {
            // `kill 0` and `kill -- -$$` (the shell's group) also reach the rest of
            // the group where the host may signal it.
            vp.sigs.raise(&vp.core, sig);
            if pid <= 0 {
                let _ = vp.host().kill(pid, sig);
            }
            continue;
        }
        // One of our jobs: the job (its externals' groups, its vprocs).
        let job = u32::try_from(pid)
            .ok()
            .and_then(|p| vp.jobs.borrow().by_pid(p));
        if let Some(j) = job
            && j.status().is_none()
        {
            j.signal(sig, &vp.core.sched);
            continue;
        }
        // The shell itself, from a shell psh runs for `bash`/`sh` (`sh -c "kill $$"`):
        // its mask, as `kill $$` there; never the process an embedder runs psh in.
        if u32::try_from(pid).is_ok_and(|p| p == vp.host().pid()) {
            crate::state::Sigs::Shell.raise(&vp.core, sig);
            continue;
        }
        if let Err(e) = vp.host().kill(pid, sig) {
            if e.kind() == std::io::ErrorKind::Unsupported {
                // A signal of psh's (Linux) numbering this system does not have (D1904).
                let name = signals::name(sig).unwrap_or_else(|| sig.to_string());
                vp.error(&format!("kill: {name}: no such signal on this system"))
                    .await;
                status = 1;
                continue;
            }
            // kill(2)'s EPERM, not open(2)'s EACCES wording.
            let text = if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Operation not permitted".to_string()
            } else {
                error_text(&e)
            };
            vp.error(&format!("kill: ({pid}) - {text}")).await;
            status = 1;
        }
    }
    Ok(status)
}

/// `kill -l [sigspec|status ...]`: the table, or each name ↔ number.
async fn list(vp: &mut Vp, words: &[Vec<u8>]) -> R {
    if words.is_empty() {
        let mut text = String::new();
        let mut col = 0;
        for n in 1..=signals::max() {
            let Some(name) = signals::name(n) else {
                continue;
            };
            if col > 0 {
                text.push('\t');
            }
            text.push_str(&format!("{n:>2}) {name}"));
            col += 1;
            if col == 5 {
                text.push('\n');
                col = 0;
            }
        }
        if col > 0 {
            text.push('\n');
        }
        return out(vp, "kill", text.as_bytes()).await;
    }
    let mut status = 0;
    let mut text = String::new();
    for w in words {
        let w = s(w).into_owned();
        if !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) {
            let mut n: i64 = w.parse().unwrap_or(-1);
            if n > 128 {
                n -= 128;
            }
            match i32::try_from(n).ok().and_then(|n| {
                if n == 0 {
                    Some("EXIT".to_string())
                } else {
                    signals::name(n).map(|s| s[3..].to_string())
                }
            }) {
                Some(name) => {
                    text.push_str(&name);
                    text.push('\n');
                }
                None => {
                    vp.error(&format!("kill: {w}: invalid signal specification"))
                        .await;
                    status = 1;
                }
            }
        } else {
            match signals::number(&w) {
                Some(n) => text.push_str(&format!("{n}\n")),
                None => {
                    vp.error(&format!("kill: {w}: invalid signal specification"))
                        .await;
                    status = 1;
                }
            }
        }
    }
    let st = out(vp, "kill", text.as_bytes()).await?;
    Ok(if status != 0 { status } else { st })
}

/// What `jobs` says about a job's state.
fn state_text(job: &Job) -> String {
    if let Some(sig) = job.stopped() {
        return signals::describe(sig);
    }
    match job.status() {
        None => "Running".to_string(),
        Some(0) => "Done".to_string(),
        Some(s) if s > 128 && signals::name(s - 128).is_some() => signals::describe(s - 128),
        Some(s) => format!("Exit {s}"),
    }
}

/// `jobs [-lnprs] [jobspec ...]`
pub(super) async fn jobs(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let (mut long, mut pids, mut running, mut stopped) = (false, false, false, false);
    let mut i = 1;
    while let Some(a) = args.get(i) {
        let a = s(a);
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            break;
        }
        for c in a[1..].chars() {
            match c {
                'l' => long = true,
                'p' => pids = true,
                'r' => running = true,
                's' => stopped = true,
                'n' | 'x' => {}
                _ => {
                    vp.error(&format!("jobs: -{c}: invalid option")).await;
                    vp.error_raw(
                        b"jobs: usage: jobs [-lnprs] [jobspec ...] or jobs -x command [args]\n",
                    )
                    .await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let mut selected: Vec<Rc<Job>> = Vec::new();
    let mut status = 0;
    if args.len() > i {
        for w in &args[i..] {
            match job_spec(vp, &s(w)) {
                Ok(j) => selected.push(j),
                Err(msg) => {
                    vp.error(&format!("jobs: {msg}")).await;
                    status = 1;
                }
            }
        }
    } else {
        selected = vp.jobs.borrow().listed();
    }
    // A job that has not started its program yet gets the chance (its pid).
    for j in &selected {
        crate::jobs::settle(&vp.core.sched, j).await;
    }
    let mut text = String::new();
    for j in selected.iter().filter(|j| !j.disowned.get()) {
        let halted = j.stopped().is_some();
        if (stopped && !halted) || (running && (halted || j.status().is_some())) {
            continue;
        }
        if pids {
            text.push_str(&format!("{}\n", j.pid()));
            continue;
        }
        let mark = vp.jobs.borrow().mark(j);
        let state = state_text(j);
        let amp = if j.status().is_none() && !halted {
            " &"
        } else {
            ""
        };
        if long {
            text.push_str(&format!(
                "[{}]{} {} {:<27}{}{}\n",
                j.id,
                mark,
                j.pid(),
                state,
                j.text,
                amp
            ));
        } else {
            text.push_str(&format!(
                "[{}]{}  {:<27}{}{}\n",
                j.id, mark, state, j.text, amp
            ));
        }
    }
    let st = out(vp, "jobs", text.as_bytes()).await?;
    // Reported finished jobs leave the table.
    for j in &selected {
        reap(vp, j);
    }
    Ok(if status != 0 { status } else { st })
}

/// `disown [-ar] [jobspec ...]`: the jobs leave the table and the shell's run.
pub(super) async fn disown(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let (mut all, mut running) = (false, false);
    let mut i = 1;
    while let Some(a) = args.get(i) {
        let a = s(a);
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            break;
        }
        for c in a[1..].chars() {
            match c {
                'a' => all = true,
                'r' => running = true,
                'h' => {}
                _ => {
                    vp.error(&format!("disown: -{c}: invalid option")).await;
                    vp.error_raw(b"disown: usage: disown [-h] [-ar] [jobspec ... | pid ...]\n")
                        .await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let mut status = 0;
    let mut chosen: Vec<Rc<Job>> = Vec::new();
    if args.len() > i {
        for w in &args[i..] {
            let w = s(w);
            let found = if w.starts_with('%') {
                job_spec(vp, &w)
            } else {
                w.parse::<u32>()
                    .ok()
                    .and_then(|p| vp.jobs.borrow().by_pid(p))
                    .ok_or_else(|| format!("{w}: no such job"))
            };
            match found {
                Ok(j) => chosen.push(j),
                Err(msg) => {
                    vp.error(&format!("disown: {msg}")).await;
                    status = 1;
                }
            }
        }
    } else if all || running {
        chosen = vp
            .jobs
            .borrow()
            .all()
            .into_iter()
            .filter(|j| !running || j.status().is_none())
            .collect();
    } else {
        let current = vp.jobs.borrow().current();
        match current {
            Some(j) => chosen.push(j),
            None => {
                vp.error("disown: current: no such job").await;
                return Ok(1);
            }
        }
    }
    for j in chosen {
        j.disown();
    }
    Ok(status)
}
