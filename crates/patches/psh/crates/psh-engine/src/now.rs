use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use psh_syntax::{AssignValue, CmdId, Command, CommandKind, Part, SimpleCommand, Word};

use crate::builtins;
use crate::exec;
use crate::expand;
use crate::options::SetOpt;
use crate::sched::LocalBoxFuture;
use crate::state::{Code, R, Unwind, Vp};

pub(crate) enum Now<'a> {
    Done(R),
    Later(LocalBoxFuture<'a, R>),
}

const BUILTINS: &[&[u8]] = &[
    b"echo",
    b"printf",
    b"test",
    b"[",
    b"true",
    b"false",
    b":",
    b"let",
    b"shift",
    b"return",
    b"break",
    b"continue",
    b"read",
];

pub(crate) fn command<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    id: CmdId,
    check: bool,
) -> Option<Now<'a>> {
    let cmd = code.ast.cmd(id);
    if !cmd.redirects.is_empty()
        || !vp.traps.map.is_empty()
        || vp.opts.get(SetOpt::Xtrace)
        || vp.opts.get(SetOpt::Posix)
        || (vp.top && vp.sigs.peek(&vp.core) != 0)
        || vp.job.as_ref().is_some_and(|j| j.killed().is_some())
    {
        return None;
    }
    match &cmd.kind {
        CommandKind::Simple(s) => simple(vp, code, cmd, s, check),
        CommandKind::Arith(w) => arith(vp, code, cmd, w, check),
        _ => None,
    }
}

fn simple<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    cmd: &'a Command,
    s: &'a SimpleCommand,
    check: bool,
) -> Option<Now<'a>> {
    if vp.exec_tail == Some(std::ptr::from_ref(cmd) as usize) {
        return None;
    }
    let builtin = match s.words.first() {
        None => {
            if vp.opts.get(SetOpt::Allexport) {
                return None;
            }
            None
        }
        Some(w) => {
            let [p] = w.parts.as_slice() else {
                return None;
            };
            if !matches!(p.kind, Part::Lit) {
                return None;
            }
            let name = code.ast.text(p.span);
            if !BUILTINS.contains(&name.as_bytes()) || vp.funcs.contains_key(name) {
                return None;
            }
            for a in &s.assigns {
                let plain = vp.vars.get(code.ast.text(a.name)).is_none_or(|v| {
                    !v.nameref
                        && !v.integer
                        && !v.readonly
                        && v.case == crate::vars::Case::None
                        && v.dynamic.is_none()
                });
                if a.append || !plain {
                    return None;
                }
            }
            Some(builtins::get(vp, name.as_bytes())?)
        }
    };
    for a in &s.assigns {
        match (&a.index, &a.value) {
            (None, AssignValue::Scalar(w)) if expand::word_now(code, w, false) => {}
            _ => return None,
        }
    }
    if !s.words.iter().all(|w| expand::word_now(code, w, true))
        || !s
            .words
            .iter()
            .all(|w| expand::params_now(vp, code, w, true))
        || !s.assigns.iter().all(|a| match &a.value {
            AssignValue::Scalar(w) => expand::params_now(vp, code, w, false),
            AssignValue::Array(_) => false,
        })
        || !vp.core.sched.pass()
    {
        return None;
    }
    vp.core.fg_signal.set(None);
    vp.line = code.line(cmd.span.start);
    let Some(b) = builtin else {
        for a in &s.assigns {
            let AssignValue::Scalar(w) = &a.value else {
                continue;
            };
            let v = match expand::single_now(vp, code, w) {
                Ok(v) => v,
                Err(msg) => return Some(arith_failed(vp, msg)),
            };
            if let Err(msg) = vp.assign_with(code.ast.text(a.name), v, a.append) {
                return Some(Now::Later(Box::pin(async move {
                    vp.error(&msg).await;
                    Err(Unwind::Abort(1))
                })));
            }
        }
        exec::set_underscore(vp, Vec::new());
        return Some(finish(vp, code, cmd, 0, check));
    };
    let argv = match expand::words_now(vp, code, &s.words) {
        Ok(argv) => argv,
        Err(msg) => return Some(arith_failed(vp, msg)),
    };
    let last = argv.last().cloned().unwrap_or_default();
    let mut saved: Vec<(&str, Option<crate::vars::Var>)> = Vec::new();
    for a in &s.assigns {
        let AssignValue::Scalar(w) = &a.value else {
            continue;
        };
        let v = match expand::single_now(vp, code, w) {
            Ok(v) => v,
            Err(msg) => {
                restore(vp, saved);
                return Some(arith_failed(vp, msg));
            }
        };
        let name = code.ast.text(a.name);
        saved.push((name, vp.vars.get(name).cloned()));
        let var = vp.vars.entry(name);
        var.set(v);
        var.exported = true;
    }
    vp.exec_now = false;
    let mut fut: LocalBoxFuture<'a, R> = Box::pin(async move {
        let r = crate::builtins::call(vp, b, &argv).await;
        restore(vp, saved);
        exec::set_underscore(vp, last);
        let status = r?;
        match finish(vp, code, cmd, status, check) {
            Now::Done(r) => r,
            Now::Later(f) => f.await,
        }
    });
    Some(
        match fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(r) => Now::Done(r),
            Poll::Pending => Now::Later(fut),
        },
    )
}

fn restore(vp: &mut Vp, saved: Vec<(&str, Option<crate::vars::Var>)>) {
    for (name, old) in saved.into_iter().rev() {
        match old {
            Some(var) => vp.vars.insert(name, var),
            None => {
                vp.vars.remove(name);
            }
        }
    }
}

fn arith_failed(vp: &mut Vp, msg: String) -> Now<'_> {
    Now::Later(Box::pin(async move {
        Err(expand::arith_failed(vp, &msg).await)
    }))
}

fn arith<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    cmd: &'a Command,
    w: &'a Word,
    check: bool,
) -> Option<Now<'a>> {
    let lit = crate::arith::literal(code, w)?;
    if !vp.core.sched.pass() {
        return None;
    }
    vp.line = code.line(cmd.span.start);
    match crate::arith::eval_literal(vp, &lit) {
        Ok(v) => Some(finish(vp, code, cmd, i32::from(v == 0), check)),
        Err(msg) => Some(Now::Later(Box::pin(async move {
            let status = match exec::arith_error(vp, msg, "((: ").await? {
                Some(v) => i32::from(v == 0),
                None => 1,
            };
            match finish(vp, code, cmd, status, check) {
                Now::Done(r) => r,
                Now::Later(f) => f.await,
            }
        }))),
    }
}

pub(crate) fn arith_word(vp: &mut Vp, code: &Code, w: &Word) -> Option<Result<i64, String>> {
    if vp.opts.get(SetOpt::Xtrace) {
        return None;
    }
    let lit = crate::arith::literal(code, w)?;
    Some(crate::arith::eval_literal(vp, &lit))
}

fn finish<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    cmd: &'a Command,
    status: i32,
    check: bool,
) -> Now<'a> {
    exec::set_pipestatus(vp, &[status]);
    match exec::after_errexit(vp, code, cmd, status, check) {
        Some(f) => Now::Later(f),
        None => Now::Done(Ok(status)),
    }
}
