//! Rewrites every span of a tree (used to map spans that run from alias text into the
//! script back onto the script).

use crate::ast::*;
use crate::span::Span;

pub(crate) fn map_spans(ast: &mut Ast, f: &mut dyn FnMut(Span) -> Span) {
    for l in &mut ast.lists {
        l.span = f(l.span);
        for ao in &mut l.items {
            ao.span = f(ao.span);
            pipeline(&mut ao.first, f);
            for (_, p) in &mut ao.rest {
                pipeline(p, f);
            }
        }
    }
    for c in &mut ast.cmds {
        c.span = f(c.span);
        match &mut c.kind {
            CommandKind::Simple(s) => {
                s.assigns.iter_mut().for_each(|a| assign(a, f));
                s.words.iter_mut().for_each(|w| word(w, f));
            }
            CommandKind::If(i) => {
                for b in &mut i.branches {
                    b.span = f(b.span);
                }
            }
            CommandKind::For(fc) | CommandKind::Select(fc) => {
                word(&mut fc.var, f);
                fc.words.iter_mut().flatten().for_each(|w| word(w, f));
            }
            CommandKind::ArithFor(a) => {
                word(&mut a.init, f);
                word(&mut a.cond, f);
                word(&mut a.step, f);
            }
            CommandKind::Case(cs) => {
                word(&mut cs.word, f);
                for item in &mut cs.items {
                    item.span = f(item.span);
                    item.patterns.iter_mut().for_each(|w| word(w, f));
                }
            }
            CommandKind::Arith(w) => word(w, f),
            CommandKind::Cond(e) => cond(e, f),
            CommandKind::Function(func) => word(&mut func.name, f),
            CommandKind::Coproc(cp) => {
                if let Some(n) = &mut cp.name {
                    word(n, f);
                }
            }
            CommandKind::Brace(_) | CommandKind::Subshell(_) | CommandKind::While(_) => {}
        }
        for r in &mut c.redirects {
            r.span = f(r.span);
            if let Some(RedirFd::Var(v)) = &mut r.fd {
                *v = f(*v);
            }
            if let RedirTarget::Word(w) = &mut r.target {
                word(w, f);
            }
        }
    }
    for h in &mut ast.heredocs {
        word(&mut h.delimiter, f);
        word(&mut h.body, f);
        if let Some(e) = &mut h.error {
            e.span = f(e.span);
        }
    }
}

fn pipeline(p: &mut Pipeline, f: &mut dyn FnMut(Span) -> Span) {
    p.span = f(p.span);
    if let Some(t) = &mut p.time {
        t.span = f(t.span);
    }
}

fn assign(a: &mut Assign, f: &mut dyn FnMut(Span) -> Span) {
    a.span = f(a.span);
    a.name = f(a.name);
    if let Some(i) = &mut a.index {
        word(i, f);
    }
    match &mut a.value {
        AssignValue::Scalar(w) => word(w, f),
        AssignValue::Array(arr) => array(arr, f),
    }
}

fn array(arr: &mut ArrayLit, f: &mut dyn FnMut(Span) -> Span) {
    arr.span = f(arr.span);
    for e in &mut arr.elems {
        e.span = f(e.span);
        if let Some(k) = &mut e.key {
            word(k, f);
        }
        word(&mut e.value, f);
    }
}

fn cond(e: &mut CondExpr, f: &mut dyn FnMut(Span) -> Span) {
    e.span = f(e.span);
    match &mut e.kind {
        CondKind::Unary { op_span, arg, .. } => {
            *op_span = f(*op_span);
            word(arg, f);
        }
        CondKind::Binary {
            op_span, lhs, rhs, ..
        } => {
            *op_span = f(*op_span);
            word(lhs, f);
            word(rhs, f);
        }
        CondKind::Word(w) => word(w, f),
        CondKind::Not(i) | CondKind::Group(i) => cond(i, f),
        CondKind::And(a, b) | CondKind::Or(a, b) => {
            cond(a, f);
            cond(b, f);
        }
    }
}

fn word(w: &mut Word, f: &mut dyn FnMut(Span) -> Span) {
    w.span = f(w.span);
    w.parts.iter_mut().for_each(|p| part(p, f));
}

fn part(p: &mut WordPart, f: &mut dyn FnMut(Span) -> Span) {
    p.span = f(p.span);
    match &mut p.kind {
        Part::Double(inner) | Part::Locale(inner) => inner.iter_mut().for_each(|p| part(p, f)),
        Part::Param(pe) => {
            pe.name_span = f(pe.name_span);
            if let Some(Subscript::Expr(w)) = &mut pe.index {
                word(w, f);
            }
            match &mut pe.op {
                ParamOp::Default { word: w, .. } => word(w, f),
                ParamOp::RemovePrefix { pattern, .. } | ParamOp::RemoveSuffix { pattern, .. } => {
                    word(pattern, f)
                }
                ParamOp::Replace {
                    pattern,
                    replacement,
                    ..
                } => {
                    word(pattern, f);
                    if let Some(r) = replacement {
                        word(r, f);
                    }
                }
                ParamOp::Case {
                    pattern: Some(p), ..
                } => word(p, f),
                ParamOp::Substring { offset, length } => {
                    word(offset, f);
                    if let Some(l) = length {
                        word(l, f);
                    }
                }
                _ => {}
            }
        }
        Part::Arith(w) => word(w, f),
        Part::Brace(b) => match &mut **b {
            Brace::Alts(alts) => alts.iter_mut().for_each(|w| word(w, f)),
            Brace::Seq { start, end, step } => {
                *start = f(*start);
                *end = f(*end);
                if let Some(s) = step {
                    *s = f(*s);
                }
            }
        },
        Part::ExtGlob { alts, .. } => alts.iter_mut().for_each(|w| word(w, f)),
        Part::Array(arr) => array(arr, f),
        _ => {}
    }
}
