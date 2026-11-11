// Ported from GoAWK's resolver (internal/resolver/resolve.go),
// Copyright (c) 2022 Ben Hoyt, MIT License (see NOTICE).

//! Variable scopes and types: every name is a local (a function parameter), a special
//! variable (`NF`, `FS`, ...) or a global, and a scalar or an array. Types are inferred
//! from use, across calls: a parameter used as an array makes the variables passed for
//! it arrays, and the other way round. Functions are visited callees first, then more
//! passes until nothing changes (mutual recursion, parameters a function only passes on).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::ast::*;
use super::lexer::{Builtin, Pos};
use super::parser::ParseError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Unknown,
    Scalar,
    Array,
}

/// The special variables, which the interpreter keeps natively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Special {
    Argc,
    Convfmt,
    Filename,
    Fnr,
    Fs,
    Nf,
    Nr,
    Ofmt,
    Ofs,
    Ors,
    Rlength,
    Rs,
    Rstart,
    Rt,
    Subsep,
}

pub fn special(name: &[u8]) -> Option<Special> {
    use Special::*;
    Some(match name {
        b"ARGC" => Argc,
        b"CONVFMT" => Convfmt,
        b"FILENAME" => Filename,
        b"FNR" => Fnr,
        b"FS" => Fs,
        b"NF" => Nf,
        b"NR" => Nr,
        b"OFMT" => Ofmt,
        b"OFS" => Ofs,
        b"ORS" => Ors,
        b"RLENGTH" => Rlength,
        b"RS" => Rs,
        b"RSTART" => Rstart,
        b"RT" => Rt,
        b"SUBSEP" => Subsep,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Local(usize),
    Special(Special),
    Global(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct VarInfo {
    pub ty: Type,
    /// Among the scalars or the arrays of its scope (assigned at the end).
    pub index: usize,
}

/// The resolver's result: what each name refers to.
pub struct Resolved {
    /// Per function (by index into `Program::functions`), its parameters.
    pub locals: Vec<HashMap<Name, VarInfo>>,
    pub globals: BTreeMap<Name, VarInfo>,
    pub funcs: HashMap<Name, usize>,
    pub num_global_scalars: usize,
    pub num_global_arrays: usize,
}

impl Resolved {
    /// What `name` is in function `func` (`None`: outside functions).
    pub fn lookup(&self, func: Option<usize>, name: &[u8]) -> (Scope, Type) {
        if let Some(f) = func
            && let Some(info) = self.locals[f].get(name)
        {
            return (Scope::Local(info.index), info.ty);
        }
        if let Some(s) = special(name) {
            return (Scope::Special(s), Type::Scalar);
        }
        let info = self.globals[name];
        (Scope::Global(info.index), info.ty)
    }
}

/// Global arrays the interpreter fills.
pub const BUILTIN_ARRAYS: &[&[u8]] = &[b"ARGV", b"ENVIRON", b"PROCINFO"];

struct Resolver<'p> {
    prog: &'p Program,
    locals: Vec<HashMap<Name, VarInfo>>,
    globals: BTreeMap<Name, VarInfo>,
    funcs: HashMap<Name, usize>,
    updates: usize,
    cur: Option<usize>,
}

type RResult<T> = Result<T, ParseError>;

/// An error gawk reports at parse time (status 1, with the source excerpt).
fn syntax(pos: Pos, msg: String) -> ParseError {
    ParseError { pos, msg }
}

/// The resolver's other errors are gawk's fatal errors (status 2, no excerpt).
fn err(pos: Pos, msg: String) -> ParseError {
    ParseError {
        pos,
        msg: format!("fatal: {msg}"),
    }
}

fn show(n: &[u8]) -> String {
    String::from_utf8_lossy(n).into_owned()
}

pub fn resolve(prog: &Program) -> RResult<Resolved> {
    let mut funcs = HashMap::new();
    for (i, f) in prog.functions.iter().enumerate() {
        if funcs.insert(f.name.clone(), i).is_some() {
            return Err(syntax(
                f.pos,
                format!("function `{}' previously defined", show(&f.name)),
            ));
        }
        if special(&f.name).is_some() || BUILTIN_ARRAYS.contains(&f.name.as_slice()) {
            return Err(err(
                f.pos,
                format!(
                    "function `{}': cannot use special variable `{}' as a function name",
                    show(&f.name),
                    show(&f.name)
                ),
            ));
        }
        for p in &f.params {
            if special(p).is_some() {
                return Err(err(
                    f.pos,
                    format!(
                        "function `{}': cannot use special variable `{}' as a function parameter",
                        show(&f.name),
                        show(p)
                    ),
                ));
            }
        }
    }
    let locals = prog
        .functions
        .iter()
        .map(|f| {
            f.params
                .iter()
                .map(|p| {
                    (
                        p.clone(),
                        VarInfo {
                            ty: Type::Unknown,
                            index: 0,
                        },
                    )
                })
                .collect()
        })
        .collect();
    let mut r = Resolver {
        prog,
        locals,
        globals: BTreeMap::new(),
        funcs,
        updates: 0,
        cur: None,
    };
    for a in BUILTIN_ARRAYS {
        r.record(a, Type::Array, Pos::default())?;
    }

    // Callees first: their parameter types are then known at the call sites.
    let order = r.call_order();
    let mut updates = r.updates;
    r.walk_all(&order)?;
    let mut rounds = 0;
    while r.updates != updates {
        updates = r.updates;
        r.walk_all(&order)?;
        rounds += 1;
        if rounds > 100 {
            return Err(err(
                Pos { line: 1, col: 1 },
                "too many iterations trying to resolve variable types".into(),
            ));
        }
    }

    // What is still unknown is a scalar (`{ f(z) } function f(x) { print NR }`), then
    // indexes: globals by name, locals by parameter order, scalars and arrays apart.
    let mut num_global_scalars = 0;
    let mut num_global_arrays = 0;
    for info in r.globals.values_mut() {
        if info.ty == Type::Unknown {
            info.ty = Type::Scalar;
        }
        if info.ty == Type::Array {
            info.index = num_global_arrays;
            num_global_arrays += 1;
        } else {
            info.index = num_global_scalars;
            num_global_scalars += 1;
        }
    }
    for (f, func) in prog.functions.iter().enumerate() {
        let (mut s, mut a) = (0, 0);
        for p in &func.params {
            let info = r.locals[f].get_mut(p).unwrap();
            if info.ty == Type::Unknown {
                info.ty = Type::Scalar;
            }
            if info.ty == Type::Array {
                info.index = a;
                a += 1;
            } else {
                info.index = s;
                s += 1;
            }
        }
    }
    Ok(Resolved {
        locals: r.locals,
        globals: r.globals,
        funcs: r.funcs,
        num_global_scalars,
        num_global_arrays,
    })
}

impl Resolver<'_> {
    /// Functions in an order where callees come before their callers (as far as
    /// cycles allow), then those no one calls.
    fn call_order(&self) -> Vec<usize> {
        let mut calls: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); self.prog.functions.len()];
        for (i, f) in self.prog.functions.iter().enumerate() {
            let mut found = Vec::new();
            for s in &f.body {
                calls_in_stmt(s, &mut found);
            }
            for name in found {
                if let Some(&j) = self.funcs.get(&name) {
                    calls[i].insert(j);
                }
            }
        }
        // Depth-first post-order.
        let mut order = Vec::new();
        let mut seen = vec![false; calls.len()];
        fn visit(i: usize, calls: &[BTreeSet<usize>], seen: &mut [bool], order: &mut Vec<usize>) {
            if seen[i] {
                return;
            }
            seen[i] = true;
            for &j in &calls[i] {
                visit(j, calls, seen, order);
            }
            order.push(i);
        }
        for i in 0..calls.len() {
            visit(i, &calls, &mut seen, &mut order);
        }
        order
    }

    fn walk_all(&mut self, order: &[usize]) -> RResult<()> {
        let prog = self.prog;
        for &f in order {
            self.cur = Some(f);
            for s in &prog.functions[f].body {
                self.stmt(s)?;
            }
        }
        self.cur = None;
        for b in &prog.begin {
            for s in b {
                self.stmt(s)?;
            }
        }
        for a in &prog.actions {
            for e in &a.pattern {
                self.expr(e)?;
            }
            if let Some(body) = &a.body {
                for s in body {
                    self.stmt(s)?;
                }
            }
        }
        for b in &prog.end {
            for s in b {
                self.stmt(s)?;
            }
        }
        Ok(())
    }

    /// Where `name` lives from the current function: (function or None for global,
    /// its info), or nothing yet.
    fn lookup(&self, name: &[u8]) -> Option<(Option<usize>, VarInfo)> {
        if let Some(f) = self.cur
            && let Some(info) = self.locals[f].get(name)
        {
            return Some((Some(f), *info));
        }
        if special(name).is_some() {
            return Some((
                None,
                VarInfo {
                    ty: Type::Scalar,
                    index: 0,
                },
            ));
        }
        self.globals.get(name).map(|i| (None, *i))
    }

    fn record(&mut self, name: &[u8], ty: Type, pos: Pos) -> RResult<()> {
        let Some((func, info)) = self.lookup(name) else {
            if self.funcs.contains_key(name) {
                return Err(syntax(
                    pos,
                    format!(
                        "function `{}' called with space between name and `(',\nor used as a variable or an array",
                        show(name)
                    ),
                ));
            }
            self.globals.insert(name.to_vec(), VarInfo { ty, index: 0 });
            self.updates += 1;
            return Ok(());
        };
        if info.ty != ty && info.ty != Type::Unknown && ty != Type::Unknown {
            let msg = if info.ty == Type::Array {
                format!("attempt to use array `{}' in a scalar context", show(name))
            } else {
                format!("attempt to use scalar `{}' as an array", show(name))
            };
            return Err(err(pos, msg));
        }
        if info.ty == Type::Unknown && ty != Type::Unknown {
            let new = VarInfo { ty, index: 0 };
            match func {
                Some(f) => {
                    self.locals[f].insert(name.to_vec(), new);
                }
                None => {
                    self.globals.insert(name.to_vec(), new);
                }
            }
            self.updates += 1;
        }
        Ok(())
    }

    fn stmts(&mut self, ss: &[Stmt]) -> RResult<()> {
        for s in ss {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> RResult<()> {
        match s {
            Stmt::Print { args, dest, .. } | Stmt::Printf { args, dest, .. } => {
                for a in args {
                    self.expr(a)?;
                }
                if let Some(d) = dest {
                    self.expr(d)?;
                }
            }
            Stmt::Expr(e) => self.expr(e)?,
            Stmt::If { cond, body, else_ } => {
                self.expr(cond)?;
                self.stmts(body)?;
                self.stmts(else_)?;
            }
            Stmt::For {
                pre,
                cond,
                post,
                body,
            } => {
                if let Some(p) = pre {
                    self.stmt(p)?;
                }
                if let Some(c) = cond {
                    self.expr(c)?;
                }
                if let Some(p) = post {
                    self.stmt(p)?;
                }
                self.stmts(body)?;
            }
            Stmt::ForIn {
                var,
                var_pos,
                array,
                array_pos,
                body,
            } => {
                self.record(var, Type::Scalar, *var_pos)?;
                self.record(array, Type::Array, *array_pos)?;
                self.stmts(body)?;
            }
            Stmt::While { cond, body } | Stmt::DoWhile { body, cond } => {
                self.expr(cond)?;
                self.stmts(body)?;
            }
            Stmt::Exit(e) | Stmt::Return(e) => {
                if let Some(e) = e {
                    self.expr(e)?;
                }
            }
            Stmt::Delete { array, pos, index } => {
                self.record(array, Type::Array, *pos)?;
                for e in index {
                    self.expr(e)?;
                }
            }
            Stmt::Block(b) => self.stmts(b)?,
            Stmt::Break | Stmt::Continue | Stmt::Next | Stmt::Nextfile | Stmt::Line(_) => {}
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> RResult<()> {
        match e {
            Expr::Num(_) | Expr::Str(_) | Expr::RegexArg(_) | Expr::Regex(_) => {}
            Expr::Field(i) | Expr::Group(i) | Expr::Unary(_, i) => self.expr(i)?,
            Expr::Var(name, pos) => {
                if self.funcs.contains_key(name)
                    && !self.cur.is_some_and(|f| self.locals[f].contains_key(name))
                {
                    return Err(syntax(
                        *pos,
                        format!(
                            "function `{}' called with space between name and `(',\nor used as a variable or an array",
                            show(name)
                        ),
                    ));
                }
                self.record(name, Type::Scalar, *pos)?
            }
            Expr::Index(name, pos, index) | Expr::In(index, name, pos) => {
                for i in index {
                    self.expr(i)?;
                }
                self.record(name, Type::Array, *pos)?;
            }
            Expr::Assign(l, r) | Expr::AugAssign(l, _, r) | Expr::Binary(l, _, r) => {
                self.expr(l)?;
                self.expr(r)?;
            }
            Expr::Cond(c, t, f) => {
                self.expr(c)?;
                self.expr(t)?;
                self.expr(f)?;
            }
            Expr::Incr { expr, .. } => self.expr(expr)?,
            Expr::Call(b, args) => match b {
                Builtin::Split => {
                    self.expr(&args[0])?;
                    if let Expr::Var(n, p) = &args[1] {
                        self.record(n, Type::Array, *p)?;
                    }
                    for a in &args[2..] {
                        self.expr(a)?;
                    }
                }
                Builtin::Match if args.len() == 3 => {
                    self.expr(&args[0])?;
                    self.expr(&args[1])?;
                    if let Expr::Var(n, p) = &args[2] {
                        self.record(n, Type::Array, *p)?;
                    }
                }
                // length(x): x may be a scalar or an array.
                Builtin::Length if matches!(args.first(), Some(Expr::Var(..))) => {
                    if let Some(Expr::Var(n, p)) = args.first() {
                        self.record(n, Type::Unknown, *p)?;
                    }
                }
                _ => {
                    for a in args {
                        self.expr(a)?;
                    }
                }
            },
            Expr::UserCall(name, pos, args) => {
                if self.cur.is_some_and(|f| self.locals[f].contains_key(name)) {
                    return Err(syntax(
                        *pos,
                        format!("function `{}' is not defined", show(name)),
                    ));
                }
                let Some(&callee) = self.funcs.get(name) else {
                    return Err(err(*pos, format!("function `{}' not defined", show(name))));
                };
                let params = self.prog.functions[callee].params.clone();
                if args.len() > params.len() {
                    return Err(err(
                        *pos,
                        format!(
                            "function `{}' called with more arguments than declared",
                            show(name)
                        ),
                    ));
                }
                for (i, arg) in args.iter().enumerate() {
                    let param = self.locals[callee][&params[i]];
                    let Expr::Var(vname, vpos) = arg else {
                        if param.ty == Type::Array {
                            return Err(err(
                                *pos,
                                format!(
                                    "function `{}': argument #{} is a scalar, not an array",
                                    show(name),
                                    i + 1
                                ),
                            ));
                        }
                        self.expr(arg)?;
                        continue;
                    };
                    let var_ty = self.lookup(vname).map_or(Type::Unknown, |(_, i)| i.ty);
                    match (var_ty, param.ty) {
                        (Type::Unknown, t) if t != Type::Unknown => {
                            self.record(vname, t, *vpos)?;
                        }
                        (t, Type::Unknown) if t != Type::Unknown => {
                            // The parameter takes the variable's type.
                            let info = self.locals[callee].get_mut(&params[i]).unwrap();
                            info.ty = t;
                            self.updates += 1;
                        }
                        (a, b) if a != b && a != Type::Unknown && b != Type::Unknown => {
                            let msg = if a == Type::Array {
                                format!(
                                    "function `{}': argument #{}: attempt to use array `{}' in a scalar context",
                                    show(name),
                                    i + 1,
                                    show(vname)
                                )
                            } else {
                                format!(
                                    "function `{}': argument #{}: attempt to use scalar `{}' as an array",
                                    show(name),
                                    i + 1,
                                    show(vname)
                                )
                            };
                            return Err(err(*vpos, msg));
                        }
                        _ => self.record(vname, Type::Unknown, *vpos)?,
                    }
                }
            }
            Expr::Getline { target, from, .. } => {
                if let Some(t) = target {
                    self.expr(t)?;
                }
                if let Some(f) = from {
                    self.expr(f)?;
                }
            }
            Expr::Multi(es, _) => {
                for e in es {
                    self.expr(e)?;
                }
            }
        }
        Ok(())
    }
}

fn calls_in_stmt(s: &Stmt, out: &mut Vec<Name>) {
    let ex = |e: &Expr, out: &mut Vec<Name>| calls_in_expr(e, out);
    match s {
        Stmt::Print { args, dest, .. } | Stmt::Printf { args, dest, .. } => {
            for a in args {
                ex(a, out);
            }
            if let Some(d) = dest {
                ex(d, out);
            }
        }
        Stmt::Expr(e) => ex(e, out),
        Stmt::If { cond, body, else_ } => {
            ex(cond, out);
            for s in body.iter().chain(else_) {
                calls_in_stmt(s, out);
            }
        }
        Stmt::For {
            pre,
            cond,
            post,
            body,
        } => {
            if let Some(p) = pre {
                calls_in_stmt(p, out);
            }
            if let Some(c) = cond {
                ex(c, out);
            }
            if let Some(p) = post {
                calls_in_stmt(p, out);
            }
            for s in body {
                calls_in_stmt(s, out);
            }
        }
        Stmt::ForIn { body, .. } | Stmt::Block(body) => {
            for s in body {
                calls_in_stmt(s, out);
            }
        }
        Stmt::While { cond, body } | Stmt::DoWhile { body, cond } => {
            ex(cond, out);
            for s in body {
                calls_in_stmt(s, out);
            }
        }
        Stmt::Exit(Some(e)) | Stmt::Return(Some(e)) => ex(e, out),
        Stmt::Delete { index, .. } => {
            for e in index {
                ex(e, out);
            }
        }
        _ => {}
    }
}

fn calls_in_expr(e: &Expr, out: &mut Vec<Name>) {
    match e {
        Expr::Field(i) | Expr::Group(i) | Expr::Unary(_, i) => calls_in_expr(i, out),
        Expr::Index(_, _, es) | Expr::In(es, _, _) | Expr::Call(_, es) | Expr::Multi(es, _) => {
            for e in es {
                calls_in_expr(e, out);
            }
        }
        Expr::Assign(l, r) | Expr::AugAssign(l, _, r) | Expr::Binary(l, _, r) => {
            calls_in_expr(l, out);
            calls_in_expr(r, out);
        }
        Expr::Cond(c, t, f) => {
            calls_in_expr(c, out);
            calls_in_expr(t, out);
            calls_in_expr(f, out);
        }
        Expr::Incr { expr, .. } => calls_in_expr(expr, out),
        Expr::UserCall(name, _, args) => {
            out.push(name.clone());
            for a in args {
                calls_in_expr(a, out);
            }
        }
        Expr::Getline { target, from, .. } => {
            if let Some(t) = target {
                calls_in_expr(t, out);
            }
            if let Some(f) = from {
                calls_in_expr(f, out);
            }
        }
        _ => {}
    }
}
