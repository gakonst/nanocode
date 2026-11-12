use std::collections::{BTreeMap, BTreeSet};

use super::eval::Native;
use super::lex::StrPart;
use super::parse::{self, BinaryOp, Cmp, Math};
use super::path::{Part, Path};
use super::val::{Val, ValR};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TermId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bind<V = (), F = V> {
    Var(V),
    Fun(F),
}

impl<T> Bind<T, T> {
    fn map<U>(&self, f: impl FnOnce(&T) -> U) -> Bind<U, U> {
        match self {
            Bind::Var(x) => Bind::Var(f(x)),
            Bind::Fun(x) => Bind::Fun(f(x)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallType {
    Inline,
    Throw,
    CatchOne,
    CatchAll,
}

#[derive(Debug)]
pub enum Pattern {
    Var,
    Index(Vec<(TermId, Pattern)>),
}

#[derive(Debug)]
pub enum Fold {
    Reduce,
    Foreach(Option<TermId>),
}

#[derive(Debug, Default)]
pub enum Term {
    #[default]
    Id,
    Empty,
    Recurse,
    ToString,
    Literal(ValR),
    Arr(TermId),
    ObjEmpty,
    ObjSingle(TermId, TermId),
    Var(usize),
    CallDef(TermId, Box<[Bind<TermId>]>, usize, CallType),
    Native(usize, Box<[Bind<TermId>]>),
    Label(TermId),
    Neg(TermId),
    Pipe(TermId, Option<Pattern>, TermId),
    Comma(TermId, TermId),
    Assign(TermId, TermId),
    Update(TermId, TermId),
    UpdateMath(TermId, Math, TermId),
    UpdateAlt(TermId, TermId),
    Logic(TermId, bool, TermId),
    Math(TermId, Math, TermId),
    Cmp(TermId, Cmp, TermId),
    Alt(TermId, TermId),
    TryCatch(TermId, TermId),
    Ite(TermId, TermId, TermId),
    Fold(TermId, Pattern, TermId, TermId, Fold),
    Path(TermId, Path<TermId>),
}

pub struct Lut {
    pub terms: Vec<Term>,
    pub natives: Vec<Native>,
}

pub struct Filter {
    pub lut: Lut,
    pub id: TermId,
}

#[derive(Clone, Copy, Debug)]
pub enum Undefined {
    Module,
    Var,
    Label,
    Filter(usize),
}

pub type NativeDef = (&'static str, Box<[Bind]>, Native);

type Tr = BTreeSet<TermId>;

enum Local<'s> {
    Arg,
    Parent(Box<[Bind<&'s str>]>, TermId),
    Sibling(Box<[Bind<&'s str>]>, TermId, Tr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Slot<'s> {
    Var(&'s str),
    Label(&'s str),
    Fun(&'s str),
}

#[derive(Default)]
struct Scope<'s> {
    bound: BTreeMap<Slot<'s>, Vec<usize>>,
    total: usize,
}

impl<'s> Scope<'s> {
    fn push(&mut self, slot: Slot<'s>) {
        self.total += 1;
        self.bound.entry(slot).or_default().push(self.total);
    }

    fn pop(&mut self, slot: Slot<'s>) {
        let stack = self.bound.get_mut(&slot).expect("a bound slot");
        assert_eq!(stack.pop(), Some(self.total));
        if stack.is_empty() {
            self.bound.remove(&slot);
        }
        self.total -= 1;
    }

    fn last(&self, slot: Slot<'s>) -> Option<usize> {
        self.bound.get(&slot)?.last().copied()
    }
}

#[derive(Default)]
struct Locals<'s> {
    funs: BTreeMap<(&'s str, usize), Vec<(Local<'s>, usize)>>,
    vars: Scope<'s>,
}

fn bind_named(name: &str) -> Bind<&str> {
    if name.starts_with('$') {
        Bind::Var(name)
    } else {
        Bind::Fun(name)
    }
}

fn binds<T, U: Copy>(params: &[Bind<T, T>], args: &[U]) -> Box<[Bind<U>]> {
    assert_eq!(params.len(), args.len());
    params
        .iter()
        .zip(args)
        .map(|(param, arg)| param.map(|_| *arg))
        .collect()
}

impl<'s> Locals<'s> {
    fn push_fun(&mut self, key: (&'s str, usize), local: Local<'s>, vars: usize) {
        self.funs.entry(key).or_default().push((local, vars));
    }

    fn pop_fun(&mut self, key: (&'s str, usize)) -> (Local<'s>, usize) {
        let stack = self.funs.get_mut(&key).expect("a bound filter");
        let top = stack.pop().expect("a bound filter");
        if stack.is_empty() {
            self.funs.remove(&key);
        }
        top
    }

    fn push_sibling(&mut self, name: &'s str, params: Box<[Bind<&'s str>]>, id: TermId, tr: Tr) {
        let vars = self.vars.total;
        self.push_fun((name, params.len()), Local::Sibling(params, id, tr), vars);
    }

    fn pop_sibling(&mut self, name: &'s str, arity: usize) -> (Box<[Bind<&'s str>]>, TermId, Tr) {
        match self.pop_fun((name, arity)) {
            (Local::Sibling(params, id, tr), vars) => {
                assert_eq!(self.vars.total, vars);
                (params, id, tr)
            }
            _ => unreachable!("the innermost {name}/{arity} is a sibling"),
        }
    }

    fn push_parent(&mut self, name: &'s str, params: Box<[Bind<&'s str>]>, id: TermId) {
        let vars = self.vars.total;
        for param in params.iter() {
            match param {
                Bind::Var(v) => self.vars.push(Slot::Var(v)),
                Bind::Fun(f) => self.push_arg(f),
            }
        }
        self.push_fun((name, params.len()), Local::Parent(params, id), vars);
    }

    fn pop_parent(&mut self, name: &'s str, arity: usize) -> TermId {
        let (params, id, vars) = match self.pop_fun((name, arity)) {
            (Local::Parent(params, id), vars) => (params, id, vars),
            _ => unreachable!("the innermost {name}/{arity} is a parent"),
        };
        for param in params.iter().rev() {
            match param {
                Bind::Var(v) => self.vars.pop(Slot::Var(v)),
                Bind::Fun(f) => self.pop_arg(f),
            }
        }
        assert_eq!(self.vars.total, vars);
        id
    }

    fn push_arg(&mut self, name: &'s str) {
        self.vars.push(Slot::Fun(name));
        let vars = self.vars.total;
        self.push_fun((name, 0), Local::Arg, vars);
    }

    fn pop_arg(&mut self, name: &'s str) {
        match self.pop_fun((name, 0)) {
            (Local::Arg, vars) => assert_eq!(self.vars.total, vars),
            _ => unreachable!("the innermost {name}/0 is an argument"),
        }
        self.vars.pop(Slot::Fun(name));
    }

    fn call(&self, name: &'s str, args: &[TermId], tr: &Tr) -> Option<(Term, Tr)> {
        let (local, vars) = self.funs.get(&(name, args.len()))?.last()?;
        let skip = self.vars.total - vars;
        Some(match local {
            Local::Arg => (Term::Var(skip), Tr::new()),
            Local::Sibling(params, id, sibling_tr) => {
                let mut sibling_tr = sibling_tr.clone();
                let recursive = sibling_tr.remove(id);
                let call = if sibling_tr.is_subset(tr) {
                    if recursive {
                        CallType::CatchOne
                    } else {
                        CallType::Inline
                    }
                } else {
                    sibling_tr = Tr::new();
                    CallType::CatchAll
                };
                (
                    Term::CallDef(*id, binds(params, args), skip, call),
                    sibling_tr,
                )
            }
            Local::Parent(params, id) => {
                let (call, parent_tr) = if tr.contains(id) {
                    (CallType::Throw, Tr::from([*id]))
                } else {
                    (CallType::CatchAll, Tr::new())
                };
                (
                    Term::CallDef(*id, binds(params, args), skip, call),
                    parent_tr,
                )
            }
        })
    }

    fn is_empty(&self) -> bool {
        self.funs.is_empty() && self.vars.bound.is_empty() && self.vars.total == 0
    }
}

struct Prelude<'s> {
    name: &'s str,
    params: Box<[Bind]>,
    id: TermId,
    tr: Tr,
}

pub struct Compiler<'s> {
    terms: Vec<Term>,
    natives: Vec<(&'static str, Box<[Bind]>)>,
    prelude: Vec<Prelude<'s>>,
    in_main: bool,
    globals: Vec<&'s str>,
    imported: Vec<(&'s str, bool)>,
    locals: Locals<'s>,
    errors: Vec<(&'s str, Undefined)>,
}

pub fn compile<'s>(
    prelude: Vec<parse::Def<'s>>,
    main: parse::Module<'s, parse::Term<'s>>,
    natives: Vec<NativeDef>,
    globals: &[&'s str],
) -> Result<Filter, Vec<(&'s str, Undefined)>> {
    let (signatures, runs): (Vec<_>, Vec<_>) = natives
        .into_iter()
        .map(|(name, params, native)| ((name, params), native))
        .unzip();
    let mut imported = Vec::new();
    for dep in &main.deps {
        if let Some(name) = dep.name.filter(|name| name.starts_with('$')) {
            imported.push((name, true));
        }
    }
    let mut compiler = Compiler {
        terms: Vec::new(),
        natives: signatures,
        prelude: Vec::new(),
        in_main: false,
        globals: globals.to_vec(),
        imported,
        locals: Locals::default(),
        errors: Vec::new(),
    };
    compiler.prelude(prelude);
    assert!(compiler.errors.is_empty(), "psh's jq definitions compile");
    assert!(compiler.locals.is_empty());
    compiler.in_main = true;
    let id = compiler.iterm(main.body);
    assert!(compiler.locals.is_empty());
    if !compiler.errors.is_empty() {
        return Err(compiler.errors);
    }
    let lut = Lut {
        terms: compiler.terms,
        natives: runs,
    };
    Ok(Filter { lut, id })
}

impl<'s> Compiler<'s> {
    fn insert(&mut self, term: Term) -> TermId {
        self.terms.push(term);
        TermId(self.terms.len() - 1)
    }

    fn prelude(&mut self, defs: Vec<parse::Def<'s>>) {
        let mut opened = Vec::with_capacity(defs.len());
        for def in defs {
            opened.push(self.open_def(def, &Tr::new()));
        }
        let mut closed = Vec::with_capacity(opened.len());
        for (name, arity) in opened.into_iter().rev() {
            let (params, id, tr) = self.locals.pop_sibling(name, arity);
            let params = params.iter().map(|p| p.map(|_| ())).collect();
            closed.push(Prelude {
                name,
                params,
                id,
                tr,
            });
        }
        closed.reverse();
        self.prelude = closed;
    }

    fn open_def(&mut self, def: parse::Def<'s>, tr: &Tr) -> (&'s str, usize) {
        let params: Box<[_]> = def.args.iter().map(|a| bind_named(a)).collect();
        let arity = params.len();
        let name = def.name;
        let (id, def_tr) = self.def(def, tr);
        self.locals.push_sibling(name, params, id, def_tr);
        (name, arity)
    }

    fn def(&mut self, def: parse::Def<'s>, tr: &Tr) -> (TermId, Tr) {
        let id = self.insert(Term::default());
        let params = def.args.iter().map(|a| bind_named(a)).collect();
        self.locals.push_parent(def.name, params, id);
        let mut tr = tr.clone();
        assert!(tr.insert(id));
        let (body, body_tr) = self.term(def.body, &tr);
        self.terms[id.0] = body;
        assert_eq!(self.locals.pop_parent(def.name, def.args.len()), id);
        (id, body_tr)
    }

    fn with_vars<T>(&mut self, vars: &[&'s str], f: impl FnOnce(&mut Self) -> T) -> T {
        for v in vars {
            self.locals.vars.push(Slot::Var(v));
        }
        let value = f(self);
        for v in vars.iter().rev() {
            self.locals.vars.pop(Slot::Var(v));
        }
        value
    }

    fn iterm(&mut self, term: parse::Term<'s>) -> TermId {
        self.iterm_tr(term, &Tr::new()).0
    }

    fn iterm_tr(&mut self, term: parse::Term<'s>, tr: &Tr) -> (TermId, Tr) {
        let id = self.insert(Term::default());
        let (term, term_tr) = self.term(term, tr);
        assert!(term_tr.is_subset(tr));
        self.terms[id.0] = term;
        (id, term_tr)
    }

    fn term(&mut self, term: parse::Term<'s>, tr: &Tr) -> (Term, Tr) {
        use parse::Term as P;
        let term = match term {
            P::Id => Term::Id,
            P::Recurse => Term::Recurse,
            P::Arr(None) => Term::Arr(self.insert(Term::Empty)),
            P::Arr(Some(t)) => Term::Arr(self.iterm(*t)),
            P::Neg(t) => Term::Neg(self.iterm(*t)),
            P::Label(name, t) => {
                self.locals.vars.push(Slot::Label(name));
                let body = self.iterm(*t);
                self.locals.vars.pop(Slot::Label(name));
                Term::Label(body)
            }
            P::Break(name) => self.break_(name),
            P::IfThenElse(branches, otherwise) => {
                return self.if_then_else(branches, otherwise, tr);
            }
            P::Var(name) => self.var(name),
            P::Call(name, args) => {
                let args: Box<[_]> = args.into_iter().map(|t| self.iterm(t)).collect();
                match name.split_once("::") {
                    Some((module, _)) => self.fail(module, Undefined::Module),
                    None => return self.call(name, &args, tr),
                }
            }
            P::Def(defs, body) => {
                let mut opened = Vec::with_capacity(defs.len());
                for def in defs {
                    opened.push(self.open_def(def, tr));
                }
                let (body, body_tr) = self.term(*body, tr);
                for (name, arity) in opened.into_iter().rev() {
                    self.locals.pop_sibling(name, arity);
                }
                return (body, body_tr);
            }
            P::Num(text) => match text.parse::<isize>() {
                Ok(n) => Term::Literal(Ok(Val::from(n))),
                Err(_) => Term::Literal(Val::from_num(text)),
            },
            P::TryCatch(body, catch) => {
                let body = self.iterm(*body);
                let catch = match catch {
                    Some(catch) => self.iterm(*catch),
                    None => self.insert(Term::Empty),
                };
                Term::TryCatch(body, catch)
            }
            P::Fold(name, source, pattern, args) => {
                return self.fold(name, *source, pattern, args, tr);
            }
            P::BinOp(l, op, r) => return self.binop(*l, op, *r, tr),
            P::Path(t, path) => {
                let t = self.iterm(*t);
                let mut parts = Vec::with_capacity(path.0.len());
                for (part, opt) in path.0 {
                    let part = match part {
                        Part::Index(i) => Part::Index(self.iterm(i)),
                        Part::Range(from, upto) => {
                            let from = from.map(|f| self.iterm(f));
                            let upto = upto.map(|u| self.iterm(u));
                            Part::Range(from, upto)
                        }
                    };
                    parts.push((part, opt));
                }
                Term::Path(t, Path(parts))
            }
            P::Str(format, parts) => self.string(format, parts),
            P::Obj(entries) => {
                let mut terms = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    terms.push(self.object_entry(key, value));
                }
                self.sum_or(Term::ObjEmpty, terms)
            }
        };
        (term, Tr::new())
    }

    fn if_then_else(
        &mut self,
        branches: Vec<(parse::Term<'s>, parse::Term<'s>)>,
        otherwise: Option<Box<parse::Term<'s>>>,
        tr: &Tr,
    ) -> (Term, Tr) {
        let mut compiled = Vec::with_capacity(branches.len());
        for (condition, then) in branches {
            let condition = self.iterm(condition);
            let then = self.iterm_tr(then, tr);
            compiled.push((condition, then));
        }
        let mut acc = match otherwise {
            Some(otherwise) => self.term(*otherwise, tr),
            None => (Term::Id, Tr::new()),
        };
        for (condition, (then, then_tr)) in compiled.into_iter().rev() {
            let otherwise = self.insert(acc.0);
            let joined = then_tr.union(&acc.1).copied().collect();
            acc = (Term::Ite(condition, then, otherwise), joined);
        }
        acc
    }

    fn fold(
        &mut self,
        name: &'s str,
        source: parse::Term<'s>,
        pattern: parse::Pattern<'s>,
        args: Vec<parse::Term<'s>>,
        tr: &Tr,
    ) -> (Term, Tr) {
        let arity = args.len();
        let mut args = args.into_iter();
        let (Some(init), Some(update)) = (args.next(), args.next()) else {
            return (self.fail(name, Undefined::Filter(arity)), Tr::new());
        };
        let mut vars = Vec::new();
        pattern.vars(&mut vars);
        let source = self.iterm(source);
        let pattern = self.pattern(pattern);
        let init = self.iterm(init);
        let update = self.with_vars(&vars, |c| c.iterm(update));
        match (name, args.next(), args.next()) {
            ("reduce", None, None) => (
                Term::Fold(source, pattern, init, update, Fold::Reduce),
                Tr::new(),
            ),
            ("foreach", project, None) => {
                let (project, project_tr) = match project {
                    Some(p) => {
                        let (p, p_tr) = self.with_vars(&vars, |c| c.iterm_tr(p, tr));
                        (Some(p), p_tr)
                    }
                    None => (None, Tr::new()),
                };
                (
                    Term::Fold(source, pattern, init, update, Fold::Foreach(project)),
                    project_tr,
                )
            }
            _ => (self.fail(name, Undefined::Filter(arity)), Tr::new()),
        }
    }

    fn binop(
        &mut self,
        l: parse::Term<'s>,
        op: BinaryOp<'s>,
        r: parse::Term<'s>,
        tr: &Tr,
    ) -> (Term, Tr) {
        let (l, (r, r_tr)) = match &op {
            BinaryOp::Comma => {
                let (l, l_tr) = self.iterm_tr(l, tr);
                let (r, r_tr) = self.iterm_tr(r, tr);
                (l, (r, l_tr.union(&r_tr).copied().collect()))
            }
            BinaryOp::Alt => (self.iterm(l), self.iterm_tr(r, tr)),
            BinaryOp::Pipe(pattern) => {
                let l = self.iterm(l);
                let mut vars = Vec::new();
                if let Some(pattern) = pattern {
                    pattern.vars(&mut vars);
                }
                (l, self.with_vars(&vars, |c| c.iterm_tr(r, tr)))
            }
            _ => (self.iterm(l), (self.iterm(r), Tr::new())),
        };
        let term = match op {
            BinaryOp::Pipe(pattern) => Term::Pipe(l, pattern.map(|p| self.pattern(p)), r),
            BinaryOp::Comma => Term::Comma(l, r),
            BinaryOp::Math(op) => Term::Math(l, op, r),
            BinaryOp::Assign => Term::Assign(l, r),
            BinaryOp::Update => Term::Update(l, r),
            BinaryOp::UpdateMath(op) => Term::UpdateMath(l, op, r),
            BinaryOp::Cmp(op) => Term::Cmp(l, op, r),
            BinaryOp::Or => Term::Logic(l, true, r),
            BinaryOp::And => Term::Logic(l, false, r),
            BinaryOp::Alt => Term::Alt(l, r),
            BinaryOp::UpdateAlt => Term::UpdateAlt(l, r),
        };
        (term, r_tr)
    }

    fn string(
        &mut self,
        format: Option<&'s str>,
        parts: Vec<StrPart<'s, parse::Term<'s>>>,
    ) -> Term {
        let plain = format.is_none();
        let format = match format {
            Some(name) => self.iterm(parse::Term::Call(name, Vec::new())),
            None => self.insert(Term::ToString),
        };
        let mut terms: Vec<Term> = Vec::with_capacity(parts.len());
        for part in parts {
            let term = match part {
                StrPart::Str(text) => Term::Literal(Ok(Val::from(text.to_string()))),
                StrPart::Char(c) => Term::Literal(Ok(Val::from(c.to_string()))),
                StrPart::Term(t) => Term::Pipe(self.iterm(t), None, format),
            };
            if plain
                && let Term::Literal(Ok(Val::Str(next))) = &term
                && let Some(Term::Literal(Ok(Val::Str(previous)))) = terms.last()
            {
                let joined = Val::str(format!("{previous}{next}"));
                *terms.last_mut().expect("a previous part") = Term::Literal(Ok(joined));
                continue;
            }
            terms.push(term);
        }
        self.sum_or(Term::Literal(Ok(Val::from(String::new()))), terms)
    }

    fn pattern(&mut self, pattern: parse::Pattern<'s>) -> Pattern {
        match pattern {
            parse::Pattern::Var(_) => Pattern::Var,
            parse::Pattern::Arr(items) => {
                let mut out = Vec::with_capacity(items.len());
                for (i, item) in items.into_iter().enumerate() {
                    let index = self.insert(Term::Literal(Ok(Val::from(i))));
                    out.push((index, self.pattern(item)));
                }
                Pattern::Index(out)
            }
            parse::Pattern::Obj(entries) => {
                let mut out = Vec::with_capacity(entries.len());
                for (key, item) in entries {
                    let key = self.iterm(key);
                    out.push((key, self.pattern(item)));
                }
                Pattern::Index(out)
            }
        }
    }

    fn fail(&mut self, name: &'s str, undefined: Undefined) -> Term {
        self.errors.push((name, undefined));
        Term::default()
    }

    fn call(&mut self, name: &'s str, args: &[TermId], tr: &Tr) -> (Term, Tr) {
        if let Some(found) = self.locals.call(name, args, tr) {
            return found;
        }
        if self.in_main {
            let found = self
                .prelude
                .iter()
                .rev()
                .find(|d| d.name == name && d.params.len() == args.len());
            if let Some(def) = found {
                assert!(def.tr.len() <= 1);
                let call = if def.tr.contains(&def.id) {
                    CallType::CatchOne
                } else {
                    CallType::Inline
                };
                let skip = self.locals.vars.total;
                return (
                    Term::CallDef(def.id, binds(&def.params, args), skip, call),
                    Tr::new(),
                );
            }
        }
        let native = self
            .natives
            .iter()
            .position(|(n, params)| *n == name && params.len() == args.len());
        if let Some(index) = native {
            let params = &self.natives[index].1;
            return (Term::Native(index, binds(params, args)), Tr::new());
        }
        (self.fail(name, Undefined::Filter(args.len())), Tr::new())
    }

    fn var(&mut self, name: &'s str) -> Term {
        let mut index = self.locals.vars.total;
        if let Some(at) = self.locals.vars.last(Slot::Var(name)) {
            return Term::Var(index - at);
        }
        for (imported, main) in self.imported.iter().rev() {
            if *imported == name && *main == self.in_main {
                return Term::Var(index);
            }
            index += 1;
        }
        for global in self.globals.iter().rev() {
            if *global == name {
                return Term::Var(index);
            }
            index += 1;
        }
        self.fail(name, Undefined::Var)
    }

    fn break_(&mut self, name: &'s str) -> Term {
        match self.locals.vars.last(Slot::Label(name)) {
            Some(at) => Term::Var(self.locals.vars.total - at),
            None => self.fail(name, Undefined::Label),
        }
    }

    fn object_entry(&mut self, key: parse::Term<'s>, value: Option<parse::Term<'s>>) -> Term {
        let (key, value) = match (key, value) {
            (parse::Term::Var(name), None) => {
                let key = self.insert(Term::Literal(Ok(Val::from(name[1..].to_string()))));
                (key, self.iterm(parse::Term::Var(name)))
            }
            (key, None) => {
                let key = self.iterm(key);
                let id = self.insert(Term::Id);
                let path = Path(Vec::from([(Part::Index(key), super::path::Opt::Essential)]));
                (key, self.insert(Term::Path(id, path)))
            }
            (key, Some(value)) => (self.iterm(key), self.iterm(value)),
        };
        Term::ObjSingle(key, value)
    }

    fn sum_or(&mut self, empty: Term, terms: Vec<Term>) -> Term {
        let mut terms = terms.into_iter().rev();
        let last = terms.next().unwrap_or(empty);
        terms.fold(last, |acc, term| {
            let l = self.insert(term);
            let r = self.insert(acc);
            Term::Math(l, Math::Add, r)
        })
    }
}
