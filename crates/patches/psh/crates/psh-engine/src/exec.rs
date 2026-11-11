//! The interpreter: lists, pipelines, commands, redirections, externals.
//!
//! Written as async functions over the AST (D1050); every `.await` is a point where the
//! vproc can wait without blocking the thread. Recursion goes through boxed futures.

use std::future::poll_fn;
use std::rc::Rc;
use std::sync::Arc;
use std::task::Poll;

use psh_syntax::{
    AndOr, AndOrOp, AssignValue, CaseTerminator, CmdId, Command, CommandKind, ListId, Pipeline,
    RedirFd, RedirOp, RedirTarget, Redirect, SimpleCommand,
};

use crate::builtins;
use crate::builtins::trap::{self, TrapKey};
use crate::expand::{self, Ctx};
use crate::fd::{Fd, FdObj, Pump};
use crate::host::{ChildFd, OpenOptions, SpawnSpec, error_text};
use crate::options::{SetOpt, Shopt};
use crate::pipe;
use crate::sched::{Join, LocalBoxFuture, safepoint};
use crate::state::{Code, Frame, Func, R, Unwind, Vp, is_name};

pub(crate) const DEFAULT_PATH: &[u8] = b"/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

pub(crate) fn run_list<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    id: ListId,
) -> LocalBoxFuture<'a, R> {
    Box::pin(async move {
        let list = code.ast.list(id);
        let mut status = vp.status;
        for (i, item) in list.items.iter().enumerate() {
            let p = &item.first;
            status = if item.background {
                Box::pin(run_background(vp, code, id, i)).await?
            } else if item.rest.is_empty()
                && p.stages.len() == 1
                && !p.negated()
                && p.time.is_none()
            {
                // The common case, without the and-or and pipeline frames: less stack
                // per nesting level (D1050).
                run_command(vp, code, p.stages[0].cmd).await?
            } else {
                Box::pin(run_andor(vp, code, item)).await?
            };
            vp.status = status;
        }
        Ok(status)
    })
}

/// `cmd &`: a job, run as a detached subshell task (D1900).
async fn run_background(vp: &mut Vp, code: &Rc<Code>, list: ListId, item: usize) -> R {
    let mut sub = enter(vp).await?;
    let code = code.clone();
    let ao = &code.ast.list(list).items[item];
    let text = code.ast.text(ao.span).trim_end();
    let text = text
        .strip_suffix('&')
        .unwrap_or(text)
        .trim_end()
        .to_string();
    // One simple command: the job is its program (bash runs it as the job's process).
    let direct = ao.rest.is_empty()
        && ao.first.stages.len() == 1
        && !ao.first.negated()
        && ao.first.time.is_none()
        && matches!(
            code.ast.cmd(ao.first.stages[0].cmd).kind,
            CommandKind::Simple(_)
        );
    let job = vp
        .jobs
        .borrow_mut()
        .add(text, vp.core.next_job_id(), direct);
    sub.job = Some(job.clone());
    vp.last_bg = Some(job.clone());
    let j = job.clone();
    let id = vp.core.sched.spawn_task(Box::pin(async move {
        let ao = &code.ast.list(list).items[item];
        // The job's own status is not errexit's or the ERR trap's to judge (only the
        // commands inside it are).
        let p = &ao.first;
        let r = if ao.rest.is_empty() && p.stages.len() == 1 && !p.negated() {
            run_command_unchecked(&mut sub, &code, p.stages[0].cmd).await
        } else {
            Box::pin(run_andor(&mut sub, &code, ao)).await
        };
        let status = r.unwrap_or_else(|u| u.status(sub.status));
        Box::pin(trap::exit_trap(&mut sub, status)).await;
        j.finish(status);
    }));
    job.set_task(id);
    Ok(0)
}

async fn run_andor(vp: &mut Vp, code: &Rc<Code>, ao: &AndOr) -> R {
    // Every pipeline but the last runs with errexit suspended (D1281).
    let n = ao.rest.len();
    let saved = vp.no_errexit;
    vp.no_errexit |= n > 0;
    let r = Box::pin(run_pipeline(vp, code, &ao.first)).await;
    vp.no_errexit = saved;
    let mut status = r?;
    for (k, (op, p)) in ao.rest.iter().enumerate() {
        vp.status = status;
        let run = match op {
            AndOrOp::And => status == 0,
            AndOrOp::Or => status != 0,
        };
        if run {
            vp.no_errexit |= k + 1 < n;
            let r = Box::pin(run_pipeline(vp, code, p)).await;
            vp.no_errexit = saved;
            status = r?;
        }
    }
    Ok(status)
}

/// `set -x` for a simple command: its prefix assignments, then its words (D1285).
async fn xtrace_command(vp: &mut Vp, assigns: &[(String, Vec<u8>)], argv: &[Vec<u8>]) {
    for (n, v) in assigns {
        let line = builtins::set::xtrace_assign(n.as_bytes(), b"=", v);
        builtins::set::xtrace(vp, &line).await;
    }
    builtins::set::xtrace(vp, &builtins::set::xtrace_line(argv)).await;
}

/// `PIPESTATUS`: the statuses of the last foreground pipeline's commands.
fn set_pipestatus(vp: &mut Vp, statuses: &[i32]) {
    let map = statuses
        .iter()
        .enumerate()
        .map(|(i, s)| (i as i64, s.to_string().into_bytes()))
        .collect();
    vp.vars.entry("PIPESTATUS").value = crate::vars::Value::Indexed(map);
}

/// errexit and the ERR trap after a command whose failure counts: a simple command
/// (with function calls), a subshell, `((…))`, `[[ … ]]`, a pipeline of several
/// commands (D1281).
pub(crate) async fn check_failure(vp: &mut Vp, status: i32) -> R {
    if status != 0 && !vp.no_errexit {
        if !vp.traps.map.is_empty() {
            Box::pin(trap::err_trap(vp, status)).await?;
        }
        if vp.opts.get(SetOpt::Errexit) {
            return Err(Unwind::Exit(status));
        }
    }
    Ok(status)
}

/// bash reports a foreground program a signal ended, other than INT and PIPE (D1303):
/// TERM as `DESCRIPTION COMMAND`, the others as `NAME: line N: PID DESCRIPTION COMMAND`.
async fn report_signal_death(vp: &mut Vp, code: &Rc<Code>, cmd: &Command, status: i32) {
    let Some((sig, pid)) = vp.core.fg_signal.take() else {
        return;
    };
    if status != 128 + sig || sig == crate::host::SIGINT || sig == crate::host::SIGPIPE {
        return;
    }
    let text = String::from_utf8_lossy(&command_text(code, cmd)).into_owned();
    let desc = crate::signals::describe(sig);
    if sig == crate::host::SIGTERM {
        vp.error_raw(format!("{desc:<27}{text}\n").as_bytes()).await;
    } else {
        vp.error(&format!("{pid:>5} {desc:<27}{text}")).await;
    }
}

/// The DEBUG trap before a command, with `BASH_COMMAND` set to its text (D1283).
async fn debug_trap(vp: &mut Vp, code: &Rc<Code>, cmd: &Command) -> Result<(), Unwind> {
    if vp.in_trap.is_some() || vp.traps.action(TrapKey::Debug).is_none_or(|a| a.is_empty()) {
        return Ok(());
    }
    let text = command_text(code, cmd);
    vp.vars.entry("BASH_COMMAND").set(text);
    vp.line = code.line(cmd.span.start);
    trap::run_trap(vp, TrapKey::Debug).await
}

/// A command's text for `BASH_COMMAND`: its source with blanks collapsed (bash prints
/// it from the tree; this is close for the commands agents write).
fn command_text(code: &Rc<Code>, cmd: &Command) -> Vec<u8> {
    let text = code.ast.text(cmd.span);
    let text = match &cmd.kind {
        // The header only: `for i in 1 2`, `case x in `.
        CommandKind::For(_) => text.split([';', '\n']).next().unwrap_or(text).trim_end(),
        CommandKind::Case(_) => text
            .find(" in")
            .and_then(|i| text.get(..i + 4))
            .unwrap_or(text),
        _ => text,
    };
    let mut out = Vec::with_capacity(text.len());
    let mut blank = false;
    for b in text.bytes() {
        if b == b' ' || b == b'\t' {
            if !blank {
                out.push(b' ');
            }
            blank = true;
        } else {
            out.push(b);
            blank = false;
        }
    }
    out
}

async fn run_pipeline(vp: &mut Vp, code: &Rc<Code>, p: &Pipeline) -> R {
    // `!` suspends errexit for the whole pipeline (D1281).
    let saved = vp.no_errexit;
    vp.no_errexit |= p.negated();
    let r = run_pipeline_stages(vp, code, p).await;
    vp.no_errexit = saved;
    let status = r?;
    Ok(if p.negated() {
        i32::from(status == 0)
    } else {
        status
    })
}

async fn run_pipeline_stages(vp: &mut Vp, code: &Rc<Code>, p: &Pipeline) -> R {
    if p.stages.is_empty() {
        return Ok(0);
    }
    if p.stages.len() == 1 {
        return run_command(vp, code, p.stages[0].cmd).await;
    }
    let n = p.stages.len();
    // `shopt -s lastpipe`: the last command runs in this shell (no job control here).
    let lastpipe = vp.opts.shopt(Shopt::Lastpipe);
    let mut joins: Vec<Join<i32>> = Vec::with_capacity(n);
    let mut next_in: Option<Fd> = None;
    let mut last_here = None;
    for (i, stage) in p.stages.iter().enumerate() {
        if i + 1 == n && lastpipe {
            let saved = vp.fds.clone();
            if let Some(r) = next_in.take() {
                vp.fds.set(0, r);
            }
            let r = run_command_unchecked(vp, code, stage.cmd).await;
            restore_fds(vp, saved);
            last_here = Some(r);
            break;
        }
        let mut sub = enter(vp).await?;
        sub.jobs = Rc::new(std::cell::RefCell::new(vp.jobs.borrow().for_stage()));
        sub.stage_subshell = matches!(code.ast.cmd(stage.cmd).kind, CommandKind::Subshell(_));
        if let Some(r) = next_in.take() {
            sub.fds.set(0, r);
        }
        if i + 1 < n {
            let (r, w) = pipe::pipe();
            let w: Fd = Rc::new(FdObj::PipeWrite(w));
            sub.fds.set(1, w.clone());
            if stage.pipe_stderr {
                sub.fds.set(2, w);
            }
            next_in = Some(Rc::new(FdObj::PipeRead(r)));
        }
        joins.push(spawn_sub(sub, code.clone(), Body::Stage(stage.cmd)));
    }
    let mut statuses = Vec::with_capacity(n);
    for j in joins {
        match crate::signals::or_signal(vp, crate::signals::Gives::Fatal, j).await {
            Ok(s) => statuses.push(s),
            // The shell ends at its next safepoint; the stages run on as tasks (D1903).
            Err(sig) => return Ok(128 + sig),
        }
    }
    if let Some(r) = last_here {
        statuses.push(r?);
    }
    let last = statuses.last().copied().unwrap_or(0);
    if let Some(stage) = p.stages.last() {
        vp.line = code.line(code.ast.cmd(stage.cmd).span.start);
    }
    // `set -o pipefail`: the last command that failed decides.
    let status = if vp.opts.get(SetOpt::Pipefail) {
        statuses
            .iter()
            .rev()
            .copied()
            .find(|&s| s != 0)
            .unwrap_or(0)
    } else {
        last
    };
    set_pipestatus(vp, &statuses);
    check_failure(vp, status).await
}

/// Clones the state for a subshell, or fails past the nesting limit (D1050).
pub(crate) async fn enter(vp: &Vp) -> Result<Vp, Unwind> {
    if vp.depth >= vp.core.max_depth {
        vp.error(&format!(
            "maximum nesting level exceeded ({})",
            vp.core.max_depth
        ))
        .await;
        return Err(Unwind::Abort(2));
    }
    Ok(vp.subshell())
}

pub(crate) enum Body {
    List(ListId),
    /// A pipeline stage: its own status is the pipeline's to judge, not errexit's.
    Stage(CmdId),
}

/// Runs a subshell as its own task; the handle yields its exit status.
pub(crate) fn spawn_sub(sub: Vp, code: Rc<Code>, body: Body) -> Join<i32> {
    let sched = sub.core.sched.clone();
    sched.spawn(async move {
        let mut sub = sub;
        let r = match body {
            Body::List(l) => run_list(&mut sub, &code, l).await,
            Body::Stage(c) => {
                let cmd = code.ast.cmd(c);
                // bash does not exec while a trap could still run in the subshell.
                let traps = sub
                    .traps
                    .map
                    .values()
                    .any(|t| t.active && !t.action.is_empty());
                if matches!(cmd.kind, CommandKind::Simple(_)) && !traps {
                    sub.exec_tail = Some(std::ptr::from_ref(cmd) as usize);
                }
                run_command_unchecked(&mut sub, &code, c).await
            }
        };
        let last = sub.status;
        let status = r.unwrap_or_else(|u| u.status(last));
        Box::pin(trap::exit_trap(&mut sub, status)).await
    })
}

pub(crate) fn run_command<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    id: CmdId,
) -> LocalBoxFuture<'a, R> {
    run_command_with(vp, code, id, true)
}

/// A command whose status errexit does not look at (a pipeline stage).
fn run_command_unchecked<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    id: CmdId,
) -> LocalBoxFuture<'a, R> {
    run_command_with(vp, code, id, false)
}

fn run_command_with<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    id: CmdId,
    check: bool,
) -> LocalBoxFuture<'a, R> {
    // Keep this body lean: its poll frame is on the stack once per nesting level
    // (D1059). Traps and errexit go through the boxed helpers below.
    Box::pin(async move {
        safepoint(&vp.core.sched).await;
        // A job sent a signal that ends processes ends here (D1900).
        if let Some(sig) = vp.job.as_ref().and_then(|j| j.killed()) {
            return Err(Unwind::Exit(128 + sig));
        }
        let cmd = code.ast.cmd(id);
        if let Some(f) = before_command(vp, code, cmd) {
            f.await?;
        }
        let mark = vp.subst_fds.len();
        let last_args = vp.last_args.len();
        if matches!(cmd.kind, CommandKind::Simple(_)) {
            vp.core.fg_signal.set(None);
        }
        let result = match &cmd.kind {
            CommandKind::Simple(s) => run_simple(vp, code, cmd, s).await,
            CommandKind::Select(_) | CommandKind::Coproc(_) => {
                vp.line = code.line(cmd.span.start);
                expand::unsupported(vp, compound_name(&cmd.kind)).await
            }
            _ if cmd.redirects.is_empty() => run_compound(vp, code, cmd).await,
            _ => Box::pin(with_redirects(vp, code, cmd)).await,
        };
        // Process substitutions of this command are closed when it completes.
        if vp.subst_fds.len() > mark {
            for n in vp.subst_fds.split_off(mark) {
                vp.fds.close(n);
            }
        }
        if vp.last_args.len() > last_args {
            let last = vp.last_args.swap_remove(last_args);
            vp.last_args.truncate(last_args);
            // bash also says `_: readonly variable` each time; an await here would
            // grow every level of a recursion (D1059), so a readonly `_` stays silent.
            set_underscore(vp, last);
        }
        let status = result?;
        match after_command(vp, code, cmd, status, check) {
            Some(f) => f.await,
            None => Ok(status),
        }
    })
}

/// `$_` after a command: its last word, not exported (bash), unless `_` is readonly.
fn set_underscore(vp: &mut Vp, last: Vec<u8>) {
    let var = vp.vars.entry("_");
    if !var.readonly {
        var.set(last);
        var.exported = false;
    }
}

/// Before a command: pending signals, the DEBUG trap (D1283, D1284).
fn before_command<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    cmd: &'a Command,
) -> Option<LocalBoxFuture<'a, Result<(), Unwind>>> {
    let signals = vp.top && vp.sigs.peek(&vp.core) != 0;
    let debug = !vp.traps.map.is_empty()
        && matches!(
            cmd.kind,
            CommandKind::Simple(_)
                | CommandKind::Arith(_)
                | CommandKind::Cond(_)
                | CommandKind::Case(_)
        );
    if !signals && !debug {
        return None;
    }
    Some(Box::pin(async move {
        if signals {
            trap::take_signals(vp).await?;
        }
        if debug {
            debug_trap(vp, code, cmd).await?;
        }
        Ok(())
    }))
}

/// After a command: `PIPESTATUS`, then errexit and the ERR trap. Compound commands
/// other than these fail only through a command inside them, which errexit has seen
/// already (D1281).
fn after_command<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    cmd: &'a Command,
    status: i32,
    check: bool,
) -> Option<LocalBoxFuture<'a, R>> {
    if !matches!(
        cmd.kind,
        CommandKind::Simple(_)
            | CommandKind::Subshell(_)
            | CommandKind::Arith(_)
            | CommandKind::Cond(_)
    ) {
        return None;
    }
    set_pipestatus(vp, &[status]);
    // A program a signal ended is reported first (D1303); through this boxed helper, so
    // the per-level frame of `run_command_with` stays small (D1059).
    if check
        && !vp.in_subst
        && matches!(cmd.kind, CommandKind::Simple(_))
        && vp
            .core
            .fg_signal
            .get()
            .is_some_and(|(sig, _)| status == 128 + sig)
    {
        return Some(Box::pin(async move {
            report_signal_death(vp, code, cmd, status).await;
            match after_errexit(vp, code, cmd, status, check) {
                Some(f) => f.await,
                None => Ok(status),
            }
        }));
    }
    after_errexit(vp, code, cmd, status, check)
}

/// errexit and the ERR trap after a command (D1281).
fn after_errexit<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    cmd: &'a Command,
    status: i32,
    check: bool,
) -> Option<LocalBoxFuture<'a, R>> {
    if !check || status == 0 || vp.no_errexit {
        return None;
    }
    // `$LINENO` in the ERR trap: this command's line, not the last line a function it
    // called ran; `$BASH_COMMAND` its text.
    vp.line = code.line(cmd.span.start);
    if vp.traps.action(TrapKey::Err).is_some() {
        let text = command_text(code, cmd);
        vp.vars.entry("BASH_COMMAND").set(text);
    }
    Some(Box::pin(check_failure(vp, status)))
}

fn compound_name(kind: &CommandKind) -> &'static str {
    match kind {
        CommandKind::If(_) => "if",
        CommandKind::While(w) if w.until => "until",
        CommandKind::While(_) => "while",
        CommandKind::For(_) => "for",
        CommandKind::Select(_) => "select",
        CommandKind::ArithFor(_) => "for ((…))",
        CommandKind::Case(_) => "case",
        CommandKind::Arith(_) => "((…))",
        CommandKind::Cond(_) => "[[ … ]]",
        CommandKind::Function(_) => "function definitions",
        CommandKind::Coproc(_) => "coproc",
        _ => "this command",
    }
}

/// Runs a compound command with its redirections applied, then restores the
/// descriptors. A failed redirection is status 1 and the body does not run.
async fn with_redirects(vp: &mut Vp, code: &Rc<Code>, cmd: &Command) -> R {
    let saved = if cmd.redirects.is_empty() {
        None
    } else {
        let saved = vp.fds.clone();
        vp.line = code.line(cmd.redirects[0].span.start);
        match Box::pin(apply_redirects(vp, code, &cmd.redirects)).await {
            Ok(true) => {}
            Ok(false) => {
                restore_fds(vp, saved);
                // The compound command failed itself: errexit and ERR see it (D1281).
                set_pipestatus(vp, &[1]);
                return check_failure(vp, 1).await;
            }
            Err(u) => {
                restore_fds(vp, saved);
                return Err(u);
            }
        }
        Some(saved)
    };
    let r = run_compound(vp, code, cmd).await;
    if let Some(saved) = saved {
        restore_fds(vp, saved);
    }
    r
}

/// Dispatches a compound command to its own boxed future: a plain function, so it adds
/// no frame of its own to the stack (D1050).
fn run_compound<'a>(vp: &'a mut Vp, code: &'a Rc<Code>, cmd: &'a Command) -> LocalBoxFuture<'a, R> {
    match &cmd.kind {
        CommandKind::Brace(list) => run_list(vp, code, *list),
        CommandKind::Subshell(list) => Box::pin(run_subshell(vp, code, *list)),
        CommandKind::If(c) => Box::pin(run_if(vp, code, c)),
        CommandKind::While(w) => Box::pin(async move {
            vp.loops += 1;
            let r = run_while(vp, code, w).await;
            vp.loops -= 1;
            r
        }),
        CommandKind::For(f) => Box::pin(run_for(vp, code, cmd, f)),
        CommandKind::ArithFor(f) => Box::pin(async move {
            vp.line = code.line(cmd.span.start);
            vp.loops += 1;
            let r = run_arith_for(vp, code, f).await;
            vp.loops -= 1;
            r
        }),
        CommandKind::Case(c) => Box::pin(run_case(vp, code, cmd, c)),
        CommandKind::Cond(e) => Box::pin(crate::cond::run(vp, code, cmd, e)),
        CommandKind::Arith(w) => Box::pin(async move {
            vp.line = code.line(cmd.span.start);
            match arith_command(vp, code, w, "((: ", false).await? {
                Some(v) => Ok(i32::from(v == 0)),
                None => Ok(1),
            }
        }),
        CommandKind::Function(f) => Box::pin(define_function(vp, code, cmd, f)),
        _ => Box::pin(std::future::ready(Ok(0))),
    }
}

async fn run_subshell(vp: &mut Vp, code: &Rc<Code>, list: ListId) -> R {
    // A pipeline stage that is this subshell: the stage's vproc is it already.
    if std::mem::take(&mut vp.stage_subshell) {
        return run_list(vp, code, list).await;
    }
    let sub = enter(vp).await?;
    let join = spawn_sub(sub, code.clone(), Body::List(list));
    // A bash subshell is a process: it runs on when the shell dies of a signal (D1903).
    Ok(
        match crate::signals::or_signal(vp, crate::signals::Gives::Fatal, join).await {
            Ok(s) => s,
            Err(sig) => 128 + sig,
        },
    )
}

async fn run_if(vp: &mut Vp, code: &Rc<Code>, c: &psh_syntax::IfCommand) -> R {
    for br in &c.branches {
        if run_condition(vp, code, br.cond).await? == 0 {
            return run_list(vp, code, br.body).await;
        }
    }
    match c.else_body {
        Some(e) => run_list(vp, code, e).await,
        None => Ok(0),
    }
}

/// The condition of `if`, `while` or `until`: errexit is suspended (D1281).
async fn run_condition(vp: &mut Vp, code: &Rc<Code>, list: ListId) -> R {
    let saved = vp.no_errexit;
    vp.no_errexit = true;
    let r = run_list(vp, code, list).await;
    vp.no_errexit = saved;
    r
}

async fn define_function(
    vp: &mut Vp,
    code: &Rc<Code>,
    cmd: &Command,
    f: &psh_syntax::FunctionDef,
) -> R {
    vp.line = code.line(cmd.span.start);
    let Some(name) = psh_syntax::literal_text(&code.ast, &f.name) else {
        vp.error(&format!(
            "`{}': not a valid identifier",
            code.ast.text(f.name.span)
        ))
        .await;
        return Ok(1);
    };
    let source = vp.msg_name();
    Rc::make_mut(&mut vp.funcs).insert(
        name,
        Rc::new(Func {
            code: code.clone(),
            body: f.body,
            // Definitions are rare: finding the command's id by address is fine.
            def: code
                .ast
                .cmd_ids()
                .find(|&id| std::ptr::eq(code.ast.cmd(id), cmd))
                .unwrap_or(f.body),
            source,
        }),
    );
    Ok(0)
}

/// What a loop does with an unwind from its body: `Some(r)` ends the loop with `r`.
fn loop_unwind(u: Unwind, status: i32) -> Option<R> {
    match u {
        Unwind::Break(n) if n > 1 => Some(Err(Unwind::Break(n - 1))),
        Unwind::Break(_) => Some(Ok(status)),
        Unwind::Continue(n) if n > 1 => Some(Err(Unwind::Continue(n - 1))),
        Unwind::Continue(_) => None,
        other => Some(Err(other)),
    }
}

async fn run_while(vp: &mut Vp, code: &Rc<Code>, w: &psh_syntax::WhileCommand) -> R {
    let mut status = 0;
    loop {
        safepoint(&vp.core.sched).await;
        let c = match run_condition(vp, code, w.cond).await {
            Ok(c) => c,
            // `continue` (or `break`) in the condition ends the loop, as in bash.
            Err(Unwind::Continue(1)) => return Ok(status),
            Err(u) => match loop_unwind(u, status) {
                Some(r) => return r,
                None => continue,
            },
        };
        if (c == 0) == w.until {
            return Ok(status);
        }
        match run_list(vp, code, w.body).await {
            Ok(s) => status = s,
            Err(u) => {
                if let Some(r) = loop_unwind(u, vp.status) {
                    return r;
                }
                status = vp.status;
            }
        }
    }
}

async fn run_for(vp: &mut Vp, code: &Rc<Code>, cmd: &Command, f: &psh_syntax::ForCommand) -> R {
    vp.line = code.line(cmd.span.start);
    let Some(name) = psh_syntax::literal_text(&code.ast, &f.var).filter(|n| is_name(n.as_bytes()))
    else {
        vp.error(&format!(
            "`{}': not a valid identifier",
            code.ast.text(f.var.span)
        ))
        .await;
        return Ok(1);
    };
    let items: Vec<Vec<u8>> = match &f.words {
        Some(words) => Box::pin(expand::expand_words(vp, code, words))
            .await?
            .out
            .into_iter()
            .map(|f| f.text)
            .collect(),
        None => vp.args.clone(),
    };
    vp.loops += 1;
    let r = run_for_items(vp, code, &name, items, f.body, cmd).await;
    vp.loops -= 1;
    r
}

async fn run_for_items(
    vp: &mut Vp,
    code: &Rc<Code>,
    name: &str,
    items: Vec<Vec<u8>>,
    body: ListId,
    cmd: &Command,
) -> R {
    let mut status = 0;
    for item in items {
        safepoint(&vp.core.sched).await;
        // The DEBUG trap and `set -x` see the loop header on every iteration.
        if !vp.traps.map.is_empty() {
            Box::pin(debug_trap(vp, code, cmd)).await?;
        }
        if vp.opts.get(SetOpt::Xtrace) {
            builtins::set::xtrace(vp, &command_text(code, cmd)).await;
        }
        if vp.vars.get(name).is_some_and(|v| v.nameref) {
            // A nameref loop variable refers to each item in turn.
            vp.vars.entry(name).set(item);
        } else if let Err(msg) = vp.assign(name, item) {
            vp.error(&msg).await;
            return Ok(1);
        }
        match run_list(vp, code, body).await {
            Ok(s) => status = s,
            Err(u) => {
                if let Some(r) = loop_unwind(u, vp.status) {
                    return r;
                }
                status = vp.status;
            }
        }
    }
    Ok(status)
}

async fn run_arith_for(vp: &mut Vp, code: &Rc<Code>, f: &psh_syntax::ArithForCommand) -> R {
    if arith_command(vp, code, &f.init, "((: ", true)
        .await?
        .is_none()
    {
        return Ok(1);
    }
    let mut status = 0;
    // `$LINENO` in the condition and step is the header's line on every round.
    let line = vp.line;
    loop {
        safepoint(&vp.core.sched).await;
        vp.line = line;
        match arith_command(vp, code, &f.cond, "((: ", true).await? {
            None => return Ok(1),
            // An empty condition is true.
            Some(0) if !f.cond.parts.is_empty() => return Ok(status),
            Some(_) => {}
        }
        match run_list(vp, code, f.body).await {
            Ok(s) => status = s,
            Err(u) => {
                if let Some(r) = loop_unwind(u, vp.status) {
                    return r;
                }
                status = vp.status;
            }
        }
        vp.line = line;
        if arith_command(vp, code, &f.step, "((: ", true)
            .await?
            .is_none()
        {
            return Ok(1);
        }
    }
}

/// Expands and evaluates an arithmetic word for a command (`((…))`, `for ((…))`):
/// errors print `PREFIX…` and give `None` (status 1), not an exit.
async fn arith_command(
    vp: &mut Vp,
    code: &Rc<Code>,
    w: &psh_syntax::Word,
    prefix: &str,
    clause: bool,
) -> Result<Option<i64>, Unwind> {
    let (text, _) = Box::pin(expand::expand_single(vp, code, w, Ctx::Word)).await?;
    if vp.opts.get(SetOpt::Xtrace) {
        // `(( x ))`: the text as written between the parentheses; a clause of
        // `for ((…))` trimmed.
        let t: &[u8] = if clause { text.trim_ascii() } else { &text };
        let mut line = b"(( ".to_vec();
        line.extend_from_slice(t);
        line.extend_from_slice(b" ))");
        builtins::set::xtrace(vp, &line).await;
    }
    match crate::arith::eval(vp, &text) {
        Ok(v) => Ok(Some(v)),
        Err(msg) => {
            vp.error(&format!("{prefix}{msg}")).await;
            Ok(None)
        }
    }
}

async fn run_case(vp: &mut Vp, code: &Rc<Code>, cmd: &Command, c: &psh_syntax::CaseCommand) -> R {
    vp.line = code.line(cmd.span.start);
    let (word, _) = Box::pin(expand::expand_single(vp, code, &c.word, Ctx::Word)).await?;
    if vp.opts.get(SetOpt::Xtrace) {
        let text = command_text(code, cmd);
        builtins::set::xtrace(vp, text.trim_ascii_end()).await;
    }
    let utf8 = vp.utf8();
    let mut status = 0;
    let mut i = 0;
    let mut fall = false;
    while i < c.items.len() {
        let item = &c.items[i];
        let mut hit = fall;
        if !hit {
            for p in &item.patterns {
                let pat = Box::pin(expand::expand_pattern(vp, code, p)).await?;
                if crate::pattern::Pattern::with(
                    &pat,
                    vp.match_opts(vp.opts.shopt(Shopt::Nocasematch)),
                )
                .matches(&word, utf8)
                {
                    hit = true;
                    break;
                }
            }
        }
        fall = false;
        if hit {
            status = match item.body {
                Some(b) => run_list(vp, code, b).await?,
                None => 0,
            };
            match item.terminator {
                CaseTerminator::Break => return Ok(status),
                CaseTerminator::FallThrough => fall = true,
                CaseTerminator::Continue => {}
            }
        }
        i += 1;
    }
    Ok(status)
}

/// Calls a function: new positional parameters and a frame for its locals.
async fn call_function(
    vp: &mut Vp,
    f: Rc<Func>,
    argv: Vec<Vec<u8>>,
    assigns: Vec<(String, Vec<u8>)>,
) -> R {
    if vp.depth >= vp.core.max_depth {
        vp.error(&format!(
            "{}: maximum function nesting level exceeded ({})",
            String::from_utf8_lossy(&argv[0]),
            vp.core.max_depth
        ))
        .await;
        return Err(Unwind::Abort(1));
    }
    let argv0 = argv[0].clone();
    let mut args: Vec<Vec<u8>> = argv.into_iter().skip(1).collect();
    std::mem::swap(&mut vp.args, &mut args);
    let loops = std::mem::take(&mut vp.loops);
    vp.depth += 1;
    // Assignments in front of a function call last for the call, like locals.
    let mut frame = Frame::default();
    for (n, _) in &assigns {
        if !frame.saved.iter().any(|(s, _)| s == n) {
            frame.saved.push((n.clone(), vp.vars.get(n).cloned()));
            frame.temps.push(n.clone());
        }
    }
    vp.frames.push(frame);
    vp.calls.push(crate::state::Call {
        func: String::from_utf8_lossy(&argv0).into_owned(),
        source: f.source.clone(),
        line: vp.line,
        sourced: false,
        args: vp.args.clone(),
    });
    for (n, v) in assigns {
        let var = vp.vars.entry(&n);
        var.set(v);
        var.exported = true;
        vp.assigned_dynamic(&n);
    }
    let hidden = trap::enter_function(vp);
    let mut r = run_command(vp, &f.code, f.body).await;
    if let Some(t) = function_return(vp, &r, hidden)
        && let Err(u) = t.await
    {
        r = Err(u);
    }
    vp.calls.pop();
    if let Some(frame) = vp.frames.pop() {
        for (n, old) in frame.saved.into_iter().rev() {
            match old {
                Some(v) => vp.vars.insert(&n, v),
                None => {
                    vp.vars.remove(&n);
                }
            }
        }
    }
    vp.depth -= 1;
    vp.loops = loops;
    vp.args = args;
    match r {
        Err(Unwind::Return(s)) => Ok(s),
        Err(Unwind::Break(_) | Unwind::Continue(_)) => Ok(vp.status),
        r => r,
    }
}

/// The RETURN trap after a function body (it sees the function's status in `$?`), and
/// the traps the call hid come back (D1283).
fn function_return<'a>(
    vp: &'a mut Vp,
    r: &R,
    hidden: Option<trap::Hidden>,
) -> Option<LocalBoxFuture<'a, Result<(), Unwind>>> {
    if vp.traps.map.is_empty() && hidden.is_none() {
        return None;
    }
    if let Ok(s) | Err(Unwind::Return(s)) = *r {
        vp.status = s;
    }
    Some(Box::pin(trap::leave_function(vp, hidden)))
}

/// Expands a simple command: `Done` when there is nothing to run (assignments and
/// redirections only, or an error), else the words and the assignments for the command.
/// The assignments of a simple command, after its words: assigned at once without a
/// command, else collected in `assigns`, their temporary values recorded in `temps`.
#[allow(clippy::too_many_arguments)]
async fn prefix_assigns(
    vp: &mut Vp,
    code: &Rc<Code>,
    s: &SimpleCommand,
    argv: &[Vec<u8>],
    assigns: &mut Vec<(String, Vec<u8>)>,
    temps: &mut Vec<(String, Option<crate::vars::Var>)>,
    subst_status: &mut Option<i32>,
) -> Result<(), Unwind> {
    for a in &s.assigns {
        let name = code.ast.text(a.name).to_string();
        let r = match (&a.index, &a.value) {
            (None, AssignValue::Scalar(w)) => {
                let (v, st) = Box::pin(expand::assign_value(vp, code, w)).await?;
                *subst_status = st.or(*subst_status);
                if argv.is_empty() {
                    if vp.opts.get(SetOpt::Xtrace) {
                        let op: &[u8] = if a.append { b"+=" } else { b"=" };
                        let line = builtins::set::xtrace_assign(name.as_bytes(), op, &v);
                        builtins::set::xtrace(vp, &line).await;
                    }
                    // Assigned at once: later assignments see earlier ones (`a=1 b=$a`).
                    vp.assign_with(&name, v, a.append)
                } else {
                    // The value with the variable's attributes (`-i` adds on `+=`); it
                    // is set for the command's duration only.
                    let target = vp.resolve(&name).into_owned();
                    vp.prepare_value_unchecked(&target, v, a.append).map(|v| {
                        temps.push((target.clone(), vp.vars.get(&target).cloned()));
                        vp.vars.entry(&target).set(v.clone());
                        assigns.push((name, v));
                    })
                }
            }
            (Some(idx), AssignValue::Scalar(w)) => {
                let (sub, st1) = Box::pin(expand::expand_single(vp, code, idx, Ctx::Word)).await?;
                let (v, st2) = Box::pin(expand::assign_value(vp, code, w)).await?;
                *subst_status = st2.or(st1).or(*subst_status);
                vp.assign_elem(&name, &sub, v, a.append)
            }
            (None, AssignValue::Array(lit)) => {
                let (items, st) = Box::pin(expand::expand_array_lit(vp, code, lit)).await?;
                *subst_status = st.or(*subst_status);
                vp.assign_array(&name, items, a.append)
            }
            (Some(idx), AssignValue::Array(_)) => Err(format!(
                "{name}[{}]: cannot assign list to array member",
                code.ast.text(idx.span)
            )),
        };
        if let Err(msg) = r {
            vp.error(&msg).await;
            return Err(Unwind::Abort(1));
        }
    }
    Ok(())
}

async fn prepare_simple(
    vp: &mut Vp,
    code: &Rc<Code>,
    cmd: &Command,
    s: &SimpleCommand,
) -> Result<Prepared, Unwind> {
    vp.line = code.line(cmd.span.start);
    let decl = s
        .words
        .first()
        .and_then(|w| psh_syntax::literal_text(&code.ast, w))
        // A function of that name shadows the builtin.
        .filter(|t| !vp.funcs.contains_key(t.as_str()))
        .and_then(|t| builtins::declare::Which::from_name(t.as_bytes()));
    let mut decl_args = None;
    let (argv, mut subst_status) = if decl.is_some() {
        let (name, args, st) = Box::pin(expand::expand_decl_args(vp, code, &s.words)).await?;
        decl_args = Some(args);
        (vec![name], st)
    } else {
        let fields = Box::pin(expand::expand_words(vp, code, &s.words)).await?;
        let st = fields.subst_status;
        (
            fields.out.into_iter().map(|f| f.text).collect::<Vec<_>>(),
            st,
        )
    };

    // Assignments, expanded after the words.
    let mut assigns: Vec<(String, Vec<u8>)> = Vec::with_capacity(s.assigns.len());
    // In front of a command, each value is visible to the later ones while they expand
    // (`A=1 B=$A cmd`, bash); the old values come back before the command runs.
    let mut temps: Vec<(String, Option<crate::vars::Var>)> = Vec::new();
    let r = Box::pin(prefix_assigns(
        vp,
        code,
        s,
        &argv,
        &mut assigns,
        &mut temps,
        &mut subst_status,
    ))
    .await;
    for (name, old) in temps.into_iter().rev() {
        match old {
            Some(v) => vp.vars.insert(&name, v),
            None => {
                vp.vars.remove(&name);
            }
        }
    }
    r?;
    // `set -a`: what the command assigned is exported (D1280).
    if argv.is_empty() && vp.opts.get(SetOpt::Allexport) {
        for a in &s.assigns {
            let name = vp.resolve(code.ast.text(a.name)).into_owned();
            vp.vars.entry(&name).exported = true;
        }
    }

    // `$_` becomes the last word once the command completes (an assignment-only
    // command clears it); `run_command_with` applies it.
    vp.last_args.push(match (&decl_args, argv.len()) {
        (Some(args), _) => match args.last() {
            Some(builtins::declare::DeclArg::Word(w)) => w.clone(),
            Some(builtins::declare::DeclArg::Assign { name, value, .. }) => {
                let mut v = name.clone();
                v.push(b'=');
                v.extend_from_slice(value);
                v
            }
            Some(builtins::declare::DeclArg::Array { name, .. }) => name.clone(),
            None => argv[0].clone(),
        },
        (None, 0) => Vec::new(),
        (None, _) => argv[argv.len() - 1].clone(),
    });
    if argv.is_empty() {
        let status = subst_status.unwrap_or(0);
        if cmd.redirects.is_empty() {
            return Ok(Prepared::Done(status));
        }
        let saved = vp.fds.clone();
        let ok = Box::pin(apply_redirects(vp, code, &cmd.redirects)).await;
        restore_fds(vp, saved);
        return Ok(Prepared::Done(if ok? { status } else { 1 }));
    }
    if let (Some(which), Some(args)) = (decl, decl_args) {
        return Ok(Prepared::Decl(which, args, assigns));
    }
    Ok(Prepared::Run(argv, assigns))
}

enum Prepared {
    Done(i32),
    Run(Vec<Vec<u8>>, Vec<(String, Vec<u8>)>),
    /// A declaration builtin with its arguments (D1262).
    Decl(
        builtins::declare::Which,
        Vec<builtins::declare::DeclArg>,
        Vec<(String, Vec<u8>)>,
    ),
}

fn run_simple<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    cmd: &'a Command,
    s: &'a SimpleCommand,
) -> LocalBoxFuture<'a, R> {
    Box::pin(async move {
        // Only the stage's own command is the vproc's last act, not what it runs or a
        // trap that runs before it (D1902).
        let tail = vp.exec_tail == Some(std::ptr::from_ref(cmd) as usize);
        if tail {
            vp.exec_tail = None;
        }
        vp.line = code.line(cmd.span.start);
        let (argv, assigns) = match Box::pin(prepare_simple(vp, code, cmd, s)).await? {
            Prepared::Done(status) => return Ok(status),
            Prepared::Run(argv, assigns) => {
                if vp.opts.get(SetOpt::Xtrace) {
                    Box::pin(xtrace_command(vp, &assigns, &argv)).await;
                }
                (argv, assigns)
            }
            Prepared::Decl(which, args, assigns) => {
                if vp.opts.get(SetOpt::Xtrace) {
                    let mut words = vec![code.ast.text(s.words[0].span).as_bytes().to_vec()];
                    for a in &args {
                        if let builtins::declare::DeclArg::Word(w)
                        | builtins::declare::DeclArg::Assign { value: w, .. } = a
                        {
                            let mut word = Vec::new();
                            if let builtins::declare::DeclArg::Assign { name, .. } = a {
                                word.extend_from_slice(name);
                                word.push(b'=');
                            }
                            word.extend_from_slice(w);
                            words.push(word);
                        }
                    }
                    Box::pin(xtrace_command(vp, &assigns, &words)).await;
                    // The declaration's own assignments follow, as in bash.
                    for a in &args {
                        if let builtins::declare::DeclArg::Assign { name, value, .. } = a {
                            let line = builtins::set::xtrace_assign(name, b"=", value);
                            builtins::set::xtrace(vp, &line).await;
                        }
                    }
                }
                // Assignments in front of a declaration builtin last for it only.
                let old: Vec<_> = assigns
                    .iter()
                    .map(|(n, _)| (n.clone(), vp.vars.get(n).cloned()))
                    .collect();
                for (n, v) in assigns {
                    vp.set_var(&n, v);
                }
                let saved = vp.fds.clone();
                let r = if cmd.redirects.is_empty()
                    || Box::pin(apply_redirects(vp, code, &cmd.redirects)).await?
                {
                    Box::pin(builtins::declare::run(vp, which, args)).await
                } else {
                    Ok(1)
                };
                restore_fds(vp, saved);
                for (n, v) in old {
                    match v {
                        Some(v) => vp.vars.insert(&n, v),
                        None => {
                            vp.vars.remove(&n);
                        }
                    }
                }
                return r;
            }
        };
        if cmd.redirects.is_empty() {
            vp.exec_now = tail;
            return run_argv(vp, argv, assigns).await;
        }

        let name = String::from_utf8_lossy(&argv[0]).into_owned();
        // `exec` without a command keeps its redirections.
        if name == "exec" && argv.len() == 1 {
            let ok = Box::pin(apply_redirects(vp, code, &cmd.redirects)).await;
            // Nothing is restored, so nothing to apply again.
            vp.var_fds.clear();
            return Ok(if ok? { 0 } else { 1 });
        }

        let saved = vp.fds.clone();
        match Box::pin(apply_redirects(vp, code, &cmd.redirects)).await {
            Ok(true) => {}
            Ok(false) => {
                restore_fds(vp, saved);
                return Ok(1);
            }
            Err(u) => {
                restore_fds(vp, saved);
                return Err(u);
            }
        }
        if tail {
            // The vproc ends with this command: nothing to restore, and the saved
            // table must not keep the external's pipe ends open.
            drop(saved);
            vp.exec_now = true;
            return run_argv(vp, argv, assigns).await;
        }
        let r = run_argv(vp, argv, assigns).await;
        restore_fds(vp, saved);
        r
    })
}

/// Runs an expanded command: a builtin or an external.
fn run_argv(
    vp: &mut Vp,
    argv: Vec<Vec<u8>>,
    assigns: Vec<(String, Vec<u8>)>,
) -> LocalBoxFuture<'_, R> {
    let name = &argv[0];
    // Functions come first, even before special builtins (bash outside POSIX mode).
    if let Some(f) = std::str::from_utf8(name)
        .ok()
        .and_then(|n| vp.funcs.get(n))
        .cloned()
    {
        vp.exec_now = false;
        return Box::pin(call_function(vp, f, argv, assigns));
    }
    if builtins::get(vp, name).is_some() {
        vp.exec_now = false;
        return Box::pin(run_builtin(vp, argv, assigns));
    }
    // Utilities after builtins, before PATH (D1080). They run in-process: never as the
    // shell's exec tail.
    Box::pin(async move {
        match crate::util::lookup(vp, &argv[0]).await {
            Some(e) => {
                vp.exec_now = false;
                crate::util::run(vp, e, argv, assigns).await
            }
            None => run_external(vp, argv, assigns).await,
        }
    })
}

pub(crate) async fn run_builtin(
    vp: &mut Vp,
    argv: Vec<Vec<u8>>,
    assigns: Vec<(String, Vec<u8>)>,
) -> R {
    let name = argv[0].clone();
    if let Some(b) = builtins::get(vp, &name) {
        // Assignments in front of a builtin, special or not (bash outside POSIX mode),
        // last for the builtin only and are exported to what it runs (`x=1 eval cmd`).
        let saved: Vec<(String, Option<crate::state::Var>)> = assigns
            .iter()
            .map(|(n, _)| (n.clone(), vp.vars.get(n).cloned()))
            .collect();
        for (n, v) in assigns {
            let var = vp.vars.entry(&n);
            var.set(v);
            var.exported = true;
            vp.assigned_dynamic(&n);
        }
        let r = (b.run)(vp, &argv).await;
        for (n, old) in saved {
            match old {
                Some(var) => vp.vars.insert(&n, var),
                None => {
                    vp.vars.remove(&n);
                }
            }
        }
        return r;
    }
    Ok(0)
}

pub(crate) async fn run_external(
    vp: &mut Vp,
    argv: Vec<Vec<u8>>,
    assigns: Vec<(String, Vec<u8>)>,
) -> R {
    run_external_with(vp, argv, assigns, ExtOpts::default()).await
}

/// How `exec` and `command` change running an external.
#[derive(Default)]
pub(crate) struct ExtOpts {
    /// `argv[0]` for the program instead of the name (`exec -a`).
    pub arg0: Option<Vec<u8>>,
    /// Only the assignments in front of the command as environment (`exec -c`).
    pub clear_env: bool,
    /// Search this instead of `PATH` (`command -p`).
    pub path: Option<Vec<u8>>,
    /// `exec`: the child does not get `_` set to its path; a shell psh runs keeps the
    /// caller's `$$`.
    pub exec: bool,
    /// `bash`/`sh` run in psh: a virtual process instead of a program (D1411).
    pub shell: bool,
}

/// Starts an external command and waits for it, pumping its pipes to engine objects.
pub(crate) async fn run_external_with(
    vp: &mut Vp,
    mut argv: Vec<Vec<u8>>,
    assigns: Vec<(String, Vec<u8>)>,
    opts: ExtOpts,
) -> R {
    if argv.is_empty() {
        return Ok(0);
    }
    let name = argv[0].clone();
    let shown = String::from_utf8_lossy(&name).into_owned();
    let program = if opts.shell {
        name.clone()
    } else if name.contains(&b'/') {
        vp.abs_path(&name)
    } else {
        // `PATH=… cmd`: the command is looked up in that PATH, past the hash table and
        // without remembering what it finds (bash).
        let temp_path = assigns.iter().find(|(n, _)| n == "PATH").map(|(_, v)| v);
        let found = match temp_path {
            Some(p) if opts.path.is_none() => builtins::hash::search(vp, &name, Some(p)).await,
            _ => builtins::hash::find(vp, &name, opts.path.as_deref(), true).await,
        };
        match found {
            Some(p) => vp.abs_path(&p),
            None => {
                vp.error(&format!("{shown}: command not found")).await;
                return Ok(127);
            }
        }
    };
    if let Some(a) = opts.arg0 {
        argv[0] = a;
    }
    let env = if opts.clear_env {
        let mut env: Vec<(Vec<u8>, Vec<u8>)> = assigns
            .iter()
            .map(|(n, v)| (n.as_bytes().to_vec(), v.clone()))
            .collect();
        env.sort();
        env
    } else {
        // A child gets `_` as the path of what it runs, as bash passes it (not
        // through `exec`).
        let mut extra = assigns.clone();
        if !opts.exec {
            extra.retain(|(n, _)| n != "_");
            extra.push(("_".to_string(), program.clone()));
        }
        vp.env(&extra)
    };
    // `trap '' SIG` passes on to children (D1284); background jobs also ignore INT and
    // QUIT, as bash's do without job control.
    let sig_ignore: Vec<i32> = (1..64)
        .filter(|&sig| {
            vp.signal_trap_ignored(sig)
                || (vp.job.is_some() && (sig == crate::host::SIGINT || sig == crate::host::SIGQUIT))
        })
        .collect();
    let spawned = if opts.shell {
        crate::util::start_nested(vp, argv, &assigns, sig_ignore, opts.exec).await?
    } else {
        match spawn_program(vp, program, argv, env, sig_ignore, &name, &shown).await {
            Ok(s) => s,
            Err(r) => return r,
        }
    };
    let mut proc = ProcGuard {
        proc: Rc::new(std::cell::RefCell::new(Some(spawned.proc))),
        core: vp.core.clone(),
        job: vp.job.clone(),
    };
    if let Some(job) = &vp.job {
        job.started(&proc.proc);
    }
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for (n, theirs) in spawned.pipes {
        let Some(ours) = vp.fd(n) else { continue };
        let theirs: Fd = Rc::new(FdObj::File(theirs));
        match &*ours {
            FdObj::PipeRead(_) => inputs.push(Pump::new(ours.clone(), theirs)),
            _ => outputs.push(Pump::new(theirs, ours.clone())),
        }
    }
    // The vproc's last act: the external has its descriptors now (bash execs it), so
    // the vproc lets go of them; a pipe the external closes is closed (D1902).
    if std::mem::take(&mut vp.exec_now) {
        vp.fds = Default::default();
    }
    let mut status = None;
    let waited = poll_fn(|cx| {
        if status.is_none() {
            for p in &mut inputs {
                let _ = p.poll(cx);
            }
            inputs.retain(|p| !p.is_done());
        }
        for p in &mut outputs {
            let _ = p.poll(cx);
        }
        outputs.retain(|p| !p.is_done());
        if status.is_none()
            && let Poll::Ready(s) = proc.poll_wait(cx)
        {
            status = Some(s);
            // The process is gone: its input pipes close, and a vproc writing to
            // them gets EPIPE instead of waiting forever.
            inputs.clear();
        }
        match status {
            // Output is complete only at EOF of its pipes (pitfall 7).
            Some(s) if outputs.is_empty() => Poll::Ready(s),
            _ => Poll::Pending,
        }
    });
    // A signal that ends the shell ends the wait; the external is abandoned (D1903).
    let st = match crate::signals::or_signal(vp, crate::signals::Gives::Fatal, waited).await {
        Ok(st) => st,
        Err(sig) => return Ok(128 + sig),
    };
    // For the report of a signal death (D1303).
    vp.core.fg_signal.set(match st {
        crate::host::ExitStatus::Signaled(sig) if !vp.in_subst => Some((sig, proc.pid())),
        _ => None,
    });
    if let Some(job) = &vp.job {
        job.ended(match st {
            crate::host::ExitStatus::Signaled(sig) => Some(sig),
            crate::host::ExitStatus::Exited(_) => None,
        });
    }
    Ok(st.code())
}

/// Spawns a program through the host, its descriptors made `ChildFd`s. `Err` is the
/// command's result when it does not start: an error status, or a script psh ran
/// itself (D1300).
async fn spawn_program(
    vp: &mut Vp,
    program: Vec<u8>,
    argv: Vec<Vec<u8>>,
    env: Vec<(Vec<u8>, Vec<u8>)>,
    sig_ignore: Vec<i32>,
    name: &[u8],
    shown: &str,
) -> Result<crate::host::Spawned, R> {
    let mut fds = Vec::new();
    let mut has = [false; 3];
    let mut seen: Vec<(u32, &Fd)> = Vec::new();
    for (n, fd) in vp.fds.iter() {
        let child = match &**fd {
            FdObj::File(f) => ChildFd::File(f.clone()),
            _ if let Some((m, _)) = seen.iter().find(|(_, o)| Rc::ptr_eq(o, fd)) => {
                ChildFd::Dup(*m)
            }
            // An OS pipe passed as it is when possible (D1068), else a pump.
            FdObj::PipeRead(r) => match r.os_end(vp.host()) {
                Some(f) => ChildFd::File(f),
                None => ChildFd::Pipe { input: true },
            },
            FdObj::PipeWrite(w) => match w.os_end(vp.host()) {
                Some(f) => ChildFd::File(f),
                None => ChildFd::Pipe { input: false },
            },
        };
        seen.push((n, fd));
        if (n as usize) < 3 {
            has[n as usize] = true;
        }
        fds.push((n, child));
    }
    for (n, h) in has.iter().enumerate() {
        if !h {
            fds.push((n as u32, ChildFd::Null));
        }
    }
    // Kept for a script psh runs itself (D1300).
    let script_argv = argv.clone();
    let spec = SpawnSpec {
        program,
        argv,
        env,
        cwd: vp.cwd.clone(),
        fds,
        sig_ignore,
        // `nohup` and `setsid` leave the shell's run (D1067), as do disowned jobs.
        detach: matches!(
            name.rsplit(|&b| b == b'/').next(),
            Some(b"nohup" | b"setsid")
        ) || vp.job.as_ref().is_some_and(|j| j.disowned.get()),
        umask: vp.umask,
        rlimits: vp.rlimits.clone(),
    };
    let program = spec.program.clone();
    match vp.host().spawn(spec) {
        Ok(s) => Ok(s),
        Err(e) if crate::host::RunInShell::is(&e) => {
            Err(Box::pin(run_in_shell(vp, &program, script_argv, shown)).await)
        }
        // Spawning a directory fails with EACCES; bash says what it is.
        Err(_) if matches!(vp.host().stat(&program, true).await, Ok(m) if m.kind == crate::host::FileKind::Dir) =>
        {
            vp.error(&format!("{shown}: Is a directory")).await;
            Err(Ok(126))
        }
        Err(e) => {
            let status = if e.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                126
            };
            vp.error(&format!("{shown}: {}", error_text(&e))).await;
            Err(Ok(status))
        }
    }
}

/// Runs a program file the host cannot start as a psh script (D1300), as bash runs a
/// file `execve` rejects: in a subshell that starts afresh, as if a new shell ran the
/// file (only exported variables, no functions, aliases, traps or options of ours), with
/// `$0` the name as written and `$@` the arguments; the descriptors, cwd and environment
/// are the command's. A file that looks binary is refused, as bash does.
async fn run_in_shell(vp: &mut Vp, program: &[u8], argv: Vec<Vec<u8>>, shown: &str) -> R {
    let file = match vp.host().open(program, OpenOptions::read()).await {
        Ok(f) => FdObj::File(f),
        Err(e) => {
            vp.error(&format!("{shown}: {}", error_text(&e))).await;
            return Ok(126);
        }
    };
    let mut source = Vec::new();
    if let Err(e) = file.read_to_end(&mut source).await {
        vp.error(&format!("{shown}: {}", error_text(&e))).await;
        return Ok(126);
    }
    drop(file);
    // bash's test: a NUL before the first newline in the first 80 bytes.
    let head = &source[..source.len().min(80)];
    let first_line = head.split(|&b| b == b'\n').next().unwrap_or(head);
    if first_line.contains(&0) {
        vp.error(&format!(
            "{shown}: cannot execute binary file: Exec format error"
        ))
        .await;
        return Ok(126);
    }
    let mut sub = enter(vp).await?;
    start_afresh(&mut sub);
    let mut argv = argv.into_iter();
    sub.arg0 = argv.next().unwrap_or_default();
    sub.args = argv.collect();
    let (status, _) = Box::pin(crate::run_script(sub, crate::Input::Text(source))).await;
    Ok(status)
}

/// Turns a subshell into a shell that starts afresh, as a new `bash` process would
/// (D1300, D1410; also `UtilCtx::spawn_shell`, D1191): its variables are built from
/// the exported ones as [`Shell::new`](crate::Shell::new) builds them from its
/// environment (`SHLVL` one higher, `IFS`, `PPID`, the dynamic ones), with no
/// functions, aliases, traps or options of ours. `$$` stays (a nested `bash` drops
/// it, D1413). Descriptors, cwd, umask and limits stay.
pub(crate) fn start_afresh(sub: &mut Vp) {
    let env = sub.env(&[]);
    let (vars, foreign) = crate::initial_vars(&*sub.core.host.clone(), &env, &sub.cwd);
    sub.vars = vars;
    sub.foreign_env = Rc::new(foreign);
    sub.subshell_level = 0;
    sub.funcs = Default::default();
    sub.aliases = Default::default();
    sub.expand_aliases = false;
    sub.traps = Default::default();
    sub.opts = crate::options::inherited(&sub.vars);
    crate::options::sync_vars(&mut sub.vars, &sub.opts);
    sub.no_errexit = false;
    sub.in_trap = None;
    sub.xtrace_level = 1;
    sub.frames.clear();
    sub.calls.clear();
    sub.loops = 0;
    sub.status = 0;
    sub.last_bg = None;
    sub.last_args.clear();
    sub.getopts = (0, 0);
    sub.hash = Default::default();
    sub.disabled = Default::default();
    sub.dirstack.clear();
}

/// Owns a running external: dropping it abandons the process (cancel), releases it
/// when the script ended normally and only its background jobs are being dropped, or
/// disowns it when its job was disowned. The process sits in a cell its job shares,
/// so `kill` can signal it.
struct ProcGuard {
    proc: Rc<crate::jobs::ProcCell>,
    core: Rc<crate::state::Core>,
    job: Option<Rc<crate::jobs::Job>>,
}

impl ProcGuard {
    fn pid(&self) -> u32 {
        self.proc
            .borrow()
            .as_ref()
            .and_then(|p| p.pid())
            .unwrap_or(0)
    }

    fn poll_wait(&mut self, cx: &mut std::task::Context<'_>) -> Poll<crate::host::ExitStatus> {
        match self.proc.borrow_mut().as_mut() {
            Some(p) => p.poll_wait(cx),
            None => Poll::Pending,
        }
    }
}

impl Drop for ProcGuard {
    fn drop(&mut self) {
        let Some(p) = self.proc.borrow_mut().take() else {
            return;
        };
        if self.job.as_ref().is_some_and(|j| j.disowned.get()) {
            p.disown();
        } else if self.core.release_on_drop.get() || self.job.as_ref().is_some_and(|j| j.releases())
        {
            p.release();
        }
    }
}

/// The default descriptor of a redirection operator.
fn default_fd(op: RedirOp) -> u32 {
    match op {
        RedirOp::Read
        | RedirOp::ReadWrite
        | RedirOp::DupRead
        | RedirOp::HereString
        | RedirOp::HereDoc
        | RedirOp::HereDocStrip => 0,
        _ => 1,
    }
}

/// Applies redirections in order. `Ok(false)`: one failed (message printed).
pub(crate) async fn apply_redirects(
    vp: &mut Vp,
    code: &Rc<Code>,
    redirects: &[Redirect],
) -> Result<bool, Unwind> {
    for r in redirects {
        if !apply_redirect(vp, code, r).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `/dev/stdin`, `/dev/stdout`, `/dev/stderr` and `/dev/fd/N` refer to the vproc's own
/// descriptors (the process's are not the shell's when it runs embedded).
pub(crate) fn dev_fd(path: &[u8]) -> Option<u32> {
    match path {
        b"/dev/stdin" => Some(0),
        b"/dev/stdout" => Some(1),
        b"/dev/stderr" => Some(2),
        _ => path
            .strip_prefix(b"/dev/fd/")
            .and_then(|n| std::str::from_utf8(n).ok())
            .and_then(|n| n.parse().ok()),
    }
}

/// Puts back a command's saved descriptors, keeping what its `{var}>` redirections did.
fn restore_fds(vp: &mut Vp, saved: crate::fd::FdTable) {
    vp.fds = saved;
    for (n, obj) in std::mem::take(&mut vp.var_fds) {
        match obj {
            Some(obj) => vp.fds.set(n, obj),
            None => vp.fds.close(n),
        }
    }
}

async fn apply_redirect(vp: &mut Vp, code: &Rc<Code>, r: &Redirect) -> Result<bool, Unwind> {
    let fd = match r.fd {
        Some(RedirFd::Number(n)) => n,
        Some(RedirFd::Var(span)) => return var_redirect(vp, code, r, span).await,
        None => default_fd(r.op),
    };
    redirect_fd(vp, code, r, fd).await
}

/// `{var}>file`: a new descriptor of 10 or more whose number goes into `var`;
/// `{var}>&-` closes the one `var` holds (D1326).
async fn var_redirect(
    vp: &mut Vp,
    code: &Rc<Code>,
    r: &Redirect,
    span: psh_syntax::Span,
) -> Result<bool, Unwind> {
    let name = code.ast.text(span).to_string();
    let closing = matches!(r.op, RedirOp::DupRead | RedirOp::DupWrite)
        && matches!(&r.target, RedirTarget::Word(w) if code.ast.text(w.span) == "-");
    if closing {
        let value = vp.var(&name).map(<[u8]>::to_vec).unwrap_or_default();
        let n = std::str::from_utf8(&value)
            .ok()
            .and_then(|v| v.parse::<u32>().ok());
        let Some(n) = n.filter(|n| vp.fd(*n).is_some()) else {
            vp.error(&format!(
                "{}: Bad file descriptor",
                String::from_utf8_lossy(&value)
            ))
            .await;
            return Ok(false);
        };
        vp.fds.close(n);
        vp.var_fds.push((n, None));
        return Ok(true);
    }
    let n = (10..).find(|n| vp.fds.get(*n).is_none()).unwrap_or(10);
    if !redirect_fd(vp, code, r, n).await? {
        return Ok(false);
    }
    if let Err(msg) = vp.assign(&name, n.to_string().into_bytes()) {
        vp.fds.close(n);
        vp.error(&msg).await;
        return Ok(false);
    }
    vp.var_fds.push((n, vp.fds.get(n).cloned()));
    Ok(true)
}

async fn redirect_fd(vp: &mut Vp, code: &Rc<Code>, r: &Redirect, fd: u32) -> Result<bool, Unwind> {
    match r.op {
        RedirOp::HereDoc | RedirOp::HereDocStrip => {
            let RedirTarget::HereDoc(id) = &r.target else {
                return Ok(true);
            };
            let doc = code.ast.heredoc(*id);
            let body = if doc.quoted {
                literal_body(code, &doc.body)
            } else {
                if let Some(e) = &doc.error {
                    vp.error(&e.message).await;
                    return Ok(false);
                }
                Box::pin(expand::expand_heredoc(vp, code, &doc.body)).await?
            };
            vp.fds
                .set(fd, Rc::new(FdObj::PipeRead(pipe::preloaded(&body))));
            return Ok(true);
        }
        RedirOp::HereString => {
            let RedirTarget::Word(w) = &r.target else {
                return Ok(true);
            };
            let (mut body, _) = Box::pin(expand::expand_single(vp, code, w, Ctx::Word)).await?;
            body.push(b'\n');
            vp.fds
                .set(fd, Rc::new(FdObj::PipeRead(pipe::preloaded(&body))));
            return Ok(true);
        }
        _ => {}
    }
    let RedirTarget::Word(word) = &r.target else {
        return Ok(true);
    };
    let Some(target) = Box::pin(expand::expand_target(vp, code, word)).await? else {
        vp.error(&format!("{}: ambiguous redirect", code.ast.text(word.span)))
            .await;
        return Ok(false);
    };
    match r.op {
        RedirOp::DupRead | RedirOp::DupWrite => {
            if target == b"-" {
                vp.fds.close(fd);
                return Ok(true);
            }
            let (num, close) = match target.strip_suffix(b"-") {
                Some(n) => (n, true),
                None => (&target[..], false),
            };
            if !num.is_empty() && num.iter().all(u8::is_ascii_digit) {
                let src: u32 = match std::str::from_utf8(num).ok().and_then(|s| s.parse().ok()) {
                    Some(n) => n,
                    None => {
                        vp.error(&format!(
                            "{}: Bad file descriptor",
                            String::from_utf8_lossy(num)
                        ))
                        .await;
                        return Ok(false);
                    }
                };
                match vp.fd(src) {
                    Some(obj) => {
                        vp.fds.set(fd, obj);
                        if close && src != fd {
                            vp.fds.close(src);
                        }
                    }
                    // `3>&3` of a closed descriptor changes nothing (bash).
                    None if src == fd => {}
                    None => {
                        // bash names the word as written when it expands to the number
                        // (`$fd: Bad file descriptor`), or the redirected descriptor
                        // when that is given (`5>&$fd`).
                        let text = code.ast.text(word.span);
                        let shown = if !text
                            .bytes()
                            .any(|b| matches!(b, b'$' | b'`' | b'"' | b'\''))
                        {
                            src.to_string()
                        } else if r.fd.is_some() {
                            fd.to_string()
                        } else {
                            text.to_string()
                        };
                        vp.error(&format!("{shown}: Bad file descriptor")).await;
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            if r.op == RedirOp::DupWrite && matches!(r.fd, None | Some(RedirFd::Number(1))) {
                // `>& file` and `1>& file` are `&> file`.
                let Some(opts) = clobber_check(vp, &target).await else {
                    return Ok(false);
                };
                return open_to(vp, &target, opts, &[1, 2]).await;
            }
            vp.error(&format!(
                "{}: ambiguous redirect",
                String::from_utf8_lossy(&target)
            ))
            .await;
            Ok(false)
        }
        RedirOp::Read => open_to(vp, &target, OpenOptions::read(), &[fd]).await,
        RedirOp::Write => {
            let Some(opts) = clobber_check(vp, &target).await else {
                return Ok(false);
            };
            open_to(vp, &target, opts, &[fd]).await
        }
        RedirOp::Clobber => open_to(vp, &target, OpenOptions::write(), &[fd]).await,
        RedirOp::Append => open_to(vp, &target, OpenOptions::append(), &[fd]).await,
        RedirOp::ReadWrite => open_to(vp, &target, OpenOptions::read_write(), &[fd]).await,
        RedirOp::WriteBoth => {
            let Some(opts) = clobber_check(vp, &target).await else {
                return Ok(false);
            };
            open_to(vp, &target, opts, &[1, 2]).await
        }
        RedirOp::AppendBoth => open_to(vp, &target, OpenOptions::append(), &[1, 2]).await,
        RedirOp::HereString | RedirOp::HereDoc | RedirOp::HereDocStrip => Ok(true),
    }
}

async fn open_to(
    vp: &mut Vp,
    target: &[u8],
    opts: OpenOptions,
    fds: &[u32],
) -> Result<bool, Unwind> {
    if let Some(src) = dev_fd(target) {
        return match vp.fd(src) {
            Some(obj) => {
                for &n in fds {
                    vp.fds.set(n, obj.clone());
                }
                Ok(true)
            }
            None => {
                vp.error(&format!("{src}: Bad file descriptor")).await;
                Ok(false)
            }
        };
    }
    if target.is_empty() {
        vp.error(": No such file or directory").await;
        return Ok(false);
    }
    let path = vp.abs_path(target);
    let opts = builtins::umask::creation_mode(vp, opts);
    // An open that waits (a FIFO) gives way to a signal that ends the shell (D1306).
    let opened = crate::signals::or_signal(
        vp,
        crate::signals::Gives::Fatal,
        vp.host().open(&path, opts),
    )
    .await;
    let opened = match opened {
        Ok(r) => r,
        // The command fails; the shell takes the signal at its next safepoint.
        Err(_) => return Ok(false),
    };
    match opened {
        Ok(f) => {
            let obj: Fd = Rc::new(FdObj::File(f));
            for &n in fds {
                vp.fds.set(n, obj.clone());
            }
            Ok(true)
        }
        Err(e) => {
            vp.error(&format!(
                "{}: {}",
                String::from_utf8_lossy(target),
                error_text(&e)
            ))
            .await;
            Ok(false)
        }
    }
}

/// `set -o noclobber` (D1280): `>` must not truncate an existing regular file. Returns
/// the options to open with, or `None` after printing the error.
async fn clobber_check(vp: &mut Vp, target: &[u8]) -> Option<OpenOptions> {
    if !vp.opts.get(SetOpt::Noclobber) || dev_fd(target).is_some() || target.is_empty() {
        return Some(OpenOptions::write());
    }
    let path = vp.abs_path(target);
    match vp.host().stat(&path, true).await {
        Ok(m) if m.kind == crate::host::FileKind::File => {
            vp.error(&format!(
                "{}: cannot overwrite existing file",
                String::from_utf8_lossy(target)
            ))
            .await;
            None
        }
        Ok(_) => Some(OpenOptions::write()),
        // Not there: create it, failing if someone else just did.
        Err(_) => Some(OpenOptions {
            create_new: true,
            ..OpenOptions::write()
        }),
    }
}

/// The body of a quoted here-document: its text, without the stripped tabs.
fn literal_body(code: &Code, body: &psh_syntax::Word) -> Vec<u8> {
    let mut out = Vec::new();
    for p in &body.parts {
        out.extend_from_slice(code.ast.bytes(p.span));
    }
    out
}

/// `$(list)`: runs the list in a subshell with stdout into a pipe, reads it to the
/// end, strips trailing newlines. Sets `$?`.
pub(crate) async fn command_subst(
    vp: &mut Vp,
    code: &Rc<Code>,
    list: ListId,
) -> Result<(Vec<u8>, i32), Unwind> {
    subst(vp, code.clone(), list).await
}

/// The trailing newlines a command substitution drops. On a host whose programs end
/// lines with `\r\n` (`Host::strips_crlf`, Windows), a `\r` before each removed newline
/// goes too (D1940).
fn strip_newlines(vp: &Vp, out: &mut Vec<u8>) {
    strip_newlines_with(out, vp.host().strips_crlf());
}

pub(crate) fn strip_newlines_with(out: &mut Vec<u8>, crlf: bool) {
    while out.last() == Some(&b'\n') {
        out.pop();
        if crlf && out.last() == Some(&b'\r') {
            out.pop();
        }
    }
}

/// `$(< file)`: the list is one command with only an input redirection.
fn read_only_redirect(code: &Code, list: ListId) -> Option<&Redirect> {
    let l = code.ast.list(list);
    let [item] = &l.items[..] else { return None };
    if item.background
        || !item.rest.is_empty()
        || item.first.stages.len() != 1
        || item.first.negated()
    {
        return None;
    }
    let cmd = code.ast.cmd(item.first.stages[0].cmd);
    let CommandKind::Simple(s) = &cmd.kind else {
        return None;
    };
    match (&s.words[..], &s.assigns[..], &cmd.redirects[..]) {
        ([], [], [r])
            if r.op == RedirOp::Read && matches!(r.fd, None | Some(RedirFd::Number(0))) =>
        {
            Some(r)
        }
        _ => None,
    }
}

async fn subst(vp: &mut Vp, code: Rc<Code>, list: ListId) -> Result<(Vec<u8>, i32), Unwind> {
    if let Some(r) = read_only_redirect(&code, list)
        && let RedirTarget::Word(w) = &r.target
    {
        // The file's contents, read by the shell itself (bash's `$(< file)`).
        vp.line = code.line(r.span.start);
        let Some(target) = expand::expand_target(vp, &code, w).await? else {
            vp.error(&format!("{}: ambiguous redirect", code.ast.text(w.span)))
                .await;
            vp.status = 1;
            return Ok((Vec::new(), 1));
        };
        let path = vp.abs_path(&target);
        let mut out = Vec::new();
        let status = match vp
            .host()
            .open(&path, crate::host::OpenOptions::read())
            .await
        {
            Ok(f) => {
                let _ = FdObj::File(f).read_to_end(&mut out).await;
                0
            }
            Err(e) => {
                vp.error(&format!(
                    "{}: {}",
                    String::from_utf8_lossy(&target),
                    error_text(&e)
                ))
                .await;
                1
            }
        };
        vp.status = status;
        out.retain(|&b| b != 0);
        strip_newlines(vp, &mut out);
        return Ok((out, status));
    }
    let mut sub = enter(vp).await?;
    // Outside POSIX mode, command substitutions do not inherit errexit unless
    // `inherit_errexit` is set (D1281).
    if !sub.opts.shopt(Shopt::InheritErrexit) && !sub.opts.get(SetOpt::Posix) {
        sub.opts.set(SetOpt::Errexit, false);
    }
    // `set -x` shows one more `+` inside (D1285).
    sub.xtrace_level += 1;
    // Its programs' signal deaths are not reported (D1303).
    sub.in_subst = true;
    let (r, w) = pipe::pipe();
    sub.fds.set(1, Rc::new(FdObj::PipeWrite(w)));
    let join = spawn_sub(sub, code, Body::List(list));
    let reader = FdObj::PipeRead(r);
    let mut out = Vec::new();
    // The substitution's output and status, unless a signal that ends the shell comes
    // first (D1306): the shell then ends at its next safepoint.
    let collected = crate::signals::or_signal(vp, crate::signals::Gives::Fatal, async {
        let _ = reader.read_to_end(&mut out).await;
        drop(reader);
        join.await
    })
    .await;
    let status = match collected {
        Ok(s) => s,
        Err(sig) => 128 + sig,
    };
    vp.status = status;
    if out.contains(&0) {
        vp.error("warning: command substitution: ignored null byte in input")
            .await;
        out.retain(|&b| b != 0);
    }
    strip_newlines(vp, &mut out);
    Ok((out, status))
}

/// `${ list; }` and `${| list; }` (bash 5.3, D1327): the list runs in this shell, its
/// stdout read by a task of its own; with `|`, the value is `$REPLY`, local to it.
pub(crate) async fn fun_subst(
    vp: &mut Vp,
    code: &Rc<Code>,
    list: ListId,
    reply: bool,
) -> Result<(Vec<u8>, i32), Unwind> {
    let saved = vp.fds.clone();
    let old_reply = if reply {
        let old = vp.vars.get("REPLY").cloned();
        vp.vars.remove("REPLY");
        Some(old)
    } else {
        None
    };
    let (r, w) = pipe::pipe();
    if !reply {
        vp.fds.set(1, Rc::new(FdObj::PipeWrite(w)));
    } else {
        drop(w);
    }
    let reader = vp.core.sched.spawn(async move {
        let reader = FdObj::PipeRead(r);
        let mut out = Vec::new();
        let _ = reader.read_to_end(&mut out).await;
        out
    });
    let r = run_list(vp, code, list).await;
    restore_fds(vp, saved);
    let mut out = reader.await;
    let value = old_reply.map(|old| {
        let v = vp.var("REPLY").map(<[u8]>::to_vec).unwrap_or_default();
        match old {
            Some(old) => vp.vars.insert("REPLY", old),
            None => {
                vp.vars.remove("REPLY");
            }
        }
        v
    });
    let status = r?;
    vp.status = status;
    match value {
        Some(v) => Ok((v, status)),
        None => {
            out.retain(|&b| b != 0);
            strip_newlines(vp, &mut out);
            Ok((out, status))
        }
    }
}

/// `<(list)` (`output` false) or `>(list)`: the list runs as a vproc on one end of a
/// pipe; the word is `/dev/fd/N` for the other end, a descriptor of this vproc (63 and
/// down, as bash) until the command completes.
pub(crate) async fn process_subst(
    vp: &mut Vp,
    code: &Rc<Code>,
    list: ListId,
    output: bool,
) -> Result<Vec<u8>, Unwind> {
    let mut sub = enter(vp).await?;
    let (r, w) = pipe::pipe();
    let (theirs, ours): (Fd, Fd) = if output {
        (Rc::new(FdObj::PipeRead(r)), Rc::new(FdObj::PipeWrite(w)))
    } else {
        (Rc::new(FdObj::PipeWrite(w)), Rc::new(FdObj::PipeRead(r)))
    };
    sub.fds.set(if output { 0 } else { 1 }, theirs);
    let n = (10..=63)
        .rev()
        .find(|n| vp.fds.get(*n).is_none())
        .unwrap_or(63);
    vp.fds.set(n, ours);
    vp.subst_fds.push(n);
    // Detached: it runs while the command reads or writes the other end.
    drop(spawn_sub(sub, code.clone(), Body::List(list)));
    Ok(format!("/dev/fd/{n}").into_bytes())
}

/// `` `text` ``: parsed when it runs, as bash does.
pub(crate) async fn command_subst_text(
    vp: &mut Vp,
    code: &Rc<Code>,
    text: &[u8],
    offset: u32,
) -> Result<(Vec<u8>, i32), Unwind> {
    let aliases = vp.expand_aliases.then_some(&vp.aliases);
    match psh_syntax::parse_bytes_with_aliases(text, psh_syntax::Options::default(), aliases) {
        Ok(ast) => {
            let root = ast.root();
            // The line table of the text itself, not of alias texts after it.
            let script = &ast.source_bytes()[..ast.script_len() as usize];
            let inner = Rc::new(Code {
                lines: Rc::new(psh_syntax::LineIndex::from_bytes(script)),
                ast: Arc::new(ast),
                line_base: code.line(offset) - 1,
            });
            subst(vp, inner, root).await
        }
        Err(e) => {
            // bash: `NAME: command substitution: line N: …`, N counted into the text.
            let start = (e.span.start as usize).min(text.len());
            let nl = text[..start].iter().filter(|&&b| b == b'\n').count() as u32;
            let msg = format!(
                "{}: command substitution: line {}: {}\n",
                vp.msg_name(),
                code.line(offset) + nl,
                e.message
            );
            vp.error_raw(msg.as_bytes()).await;
            vp.status = 2;
            Ok((Vec::new(), 2))
        }
    }
}
