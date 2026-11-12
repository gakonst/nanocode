use core::cmp::Ordering;
use core::ops::ControlFlow;
use std::rc::Rc;

use super::compile::{Bind, CallType, Fold, NativeDef, Pattern, Term, TermId};
use super::funs::Data;
use super::iter::{
    BoxIter, box_once, collect_if_once, flat_map_then, flat_map_then_with, flat_map_with, lazy,
    map_with, then,
};
use super::list::{Lazy, List};
use super::parse::{Cmp, Math};
use super::path::{self, Opt, PathList, path_error};
use super::val::{Error, Val};

pub const EVAL_DEPTH_MAX: usize = 1024;

#[derive(Clone, Debug)]
pub struct Exn(Inner);

#[derive(Clone, Debug)]
enum Inner {
    Err(Box<Error>),
    TailCall(Box<(TermId, Vars, CallInput)>),
    Break(usize),
    Halt(i32),
    Depth,
}

#[derive(Clone, Debug)]
enum CallInput {
    Run(Val),
    Paths((Val, PathList)),
}

impl CallInput {
    fn into_run(self) -> Val {
        match self {
            CallInput::Run(v) => v,
            CallInput::Paths(_) => unreachable!("a tail call of a run is caught by a run"),
        }
    }

    fn into_paths(self) -> (Val, PathList) {
        match self {
            CallInput::Paths(vp) => vp,
            CallInput::Run(_) => unreachable!("a tail call of paths is caught by paths"),
        }
    }
}

impl Exn {
    pub fn get_err(self) -> Result<Error, Exn> {
        match self.0 {
            Inner::Err(e) => Ok(*e),
            _ => Err(self),
        }
    }

    pub fn get_halt(self) -> Result<i32, Exn> {
        match self.0 {
            Inner::Halt(code) => Ok(code),
            _ => Err(self),
        }
    }

    pub fn is_depth(&self) -> bool {
        matches!(self.0, Inner::Depth)
    }

    pub fn halt(code: i32) -> Exn {
        Exn(Inner::Halt(code))
    }

    fn depth() -> Exn {
        Exn(Inner::Depth)
    }
}

impl From<Error> for Exn {
    fn from(e: Error) -> Exn {
        Exn(Inner::Err(Box::new(e)))
    }
}

pub type ValX = Result<Val, Exn>;
pub type ValXs<'a> = BoxIter<'a, ValX>;

#[derive(Clone, Debug)]
enum Binding {
    Var(Val),
    Label(usize),
    Fun(TermId, Vars),
}

#[derive(Clone, Debug, Default)]
pub struct Vars(List<Binding>);

impl Vars {
    pub fn new(values: impl IntoIterator<Item = Val>) -> Vars {
        let mut list = List::default();
        for v in values {
            list = list.cons(Binding::Var(v));
        }
        Vars(list)
    }
}

#[derive(Clone)]
pub struct Ctx<'a> {
    data: &'a Data<'a>,
    vars: Vars,
    labels: usize,
    depth: usize,
}

pub type Cv<'a> = (Ctx<'a>, Val);
pub type Cvp<'a> = (Ctx<'a>, (Val, PathList));
pub type ValPathX = Result<(Val, PathList), Exn>;
pub type ValPathXs<'a> = BoxIter<'a, ValPathX>;
pub type Updater<'a> = Rc<dyn Fn(Val) -> ValXs<'a> + 'a>;

pub type RunPtr = for<'a> fn(Cv<'a>) -> ValXs<'a>;
pub type PathsPtr = for<'a> fn(Cvp<'a>) -> ValPathXs<'a>;

#[derive(Clone, Copy)]
pub struct Native {
    pub run: RunPtr,
    pub paths: Option<PathsPtr>,
}

impl<'a> Ctx<'a> {
    pub fn new(data: &'a Data<'a>, vars: Vars) -> Self {
        Ctx {
            data,
            vars,
            labels: 0,
            depth: 0,
        }
    }

    pub fn data(&self) -> &'a Data<'a> {
        self.data
    }

    fn cons_var(mut self, x: Val) -> Self {
        self.vars.0 = self.vars.0.cons(Binding::Var(x));
        self
    }

    fn cons_fun(mut self, id: TermId, closure: Ctx<'a>) -> Self {
        self.vars.0 = self.vars.0.cons(Binding::Fun(id, closure.vars));
        self
    }

    fn cons_label(mut self) -> Self {
        self.labels += 1;
        self.vars.0 = self.vars.0.cons(Binding::Label(self.labels));
        self
    }

    fn skip_vars(mut self, count: usize) -> Self {
        if count > 0 {
            self.vars.0 = self.vars.0.skip(count).clone();
        }
        self
    }

    fn with_vars(&self, vars: Vars) -> Self {
        Ctx {
            data: self.data,
            vars,
            labels: self.labels,
            depth: self.depth,
        }
    }

    fn term(&self, id: TermId) -> &'a Term {
        &self.data.lut.terms[id.0]
    }

    fn enter(mut self) -> Option<Self> {
        self.depth += 1;
        (self.depth <= EVAL_DEPTH_MAX).then_some(self)
    }

    fn binding(&self, index: usize) -> &Binding {
        self.vars
            .0
            .get(index)
            .expect("a variable index within its context")
    }

    pub fn pop_var(&mut self) -> Val {
        match core::mem::take(&mut self.vars.0).pop() {
            Some((Binding::Var(head), tail)) => {
                self.vars.0 = tail;
                head
            }
            _ => unreachable!("a native's variable argument is bound"),
        }
    }

    pub fn pop_fun(&mut self) -> (TermId, Ctx<'a>) {
        match core::mem::take(&mut self.vars.0).pop() {
            Some((Binding::Fun(id, vars), tail)) => {
                self.vars.0 = tail;
                (id, self.with_vars(vars))
            }
            _ => unreachable!("a native's filter argument is bound"),
        }
    }
}

fn bind_vars<'a, T: Clone + 'a>(
    args: &'a [Bind<TermId>],
    ctx: Ctx<'a>,
    cv: (Ctx<'a>, T),
    project: fn(&T) -> Val,
) -> BoxIter<'a, Result<(Ctx<'a>, T), Exn>> {
    match args.split_first() {
        Some((Bind::Var(arg), [])) => map_with(
            arg.run((cv.0.clone(), project(&cv.1))),
            (ctx, cv.1),
            |y, (ctx, v)| Ok((ctx.cons_var(y?), v)),
        ),
        Some((Bind::Fun(arg), [])) => box_once(Ok((ctx.cons_fun(*arg, cv.0), cv.1))),
        Some((Bind::Var(arg), rest)) => flat_map_then_with(
            arg.run((cv.0.clone(), project(&cv.1))),
            (ctx, cv),
            move |y, (ctx, cv)| bind_vars(rest, ctx.cons_var(y), cv, project),
        ),
        Some((Bind::Fun(arg), rest)) => {
            let ctx = ctx.cons_fun(*arg, cv.0.clone());
            bind_vars(rest, ctx, cv, project)
        }
        None => box_once(Ok((ctx, cv.1))),
    }
}

type Ctxs<'a> = BoxIter<'a, Result<Ctx<'a>, Exn>>;

fn bind_pattern<'a>((index, pattern): &'a (TermId, Pattern), ctx: Ctx<'a>, cv: Cv<'a>) -> Ctxs<'a> {
    let (ctx0, v0) = cv.clone();
    let values = map_with(index.run(cv), v0, |i, v0| Ok(v0.index(&i?)?));
    match pattern {
        Pattern::Var => Box::new(values.map(move |v| Ok(ctx.clone().cons_var(v?)))),
        Pattern::Index(patterns) => {
            flat_map_then_with(values, (ctx, ctx0), move |v, (ctx, ctx0)| {
                bind_patterns(patterns, ctx, (ctx0, v))
            })
        }
    }
}

fn bind_patterns<'a>(patterns: &'a [(TermId, Pattern)], ctx: Ctx<'a>, cv: Cv<'a>) -> Ctxs<'a> {
    match patterns.split_first() {
        None => box_once(Ok(ctx)),
        Some((pattern, [])) => bind_pattern(pattern, ctx, cv),
        Some((pattern, rest)) => {
            flat_map_then_with(bind_pattern(pattern, ctx, cv.clone()), cv, |ctx, cv| {
                bind_patterns(rest, ctx, cv)
            })
        }
    }
}

fn run_and_bind<'a>(source: TermId, cv: Cv<'a>, pattern: &'a Pattern) -> Ctxs<'a> {
    let values = source.run((cv.0.clone(), cv.1));
    match pattern {
        Pattern::Var => map_with(values, cv.0, |y, ctx| Ok(ctx.cons_var(y?))),
        Pattern::Index(patterns) => flat_map_then_with(values, cv.0, |y, ctx| {
            bind_patterns(patterns, ctx.clone(), (ctx, y))
        }),
    }
}

type RunFn<'a, T> = fn(TermId, (Ctx<'a>, T)) -> BoxIter<'a, Result<T, Exn>>;

fn bind_run<'a, T: Clone + 'a>(
    pattern: &'a Pattern,
    body: TermId,
    cv: (Ctx<'a>, T),
    y: Val,
    run: RunFn<'a, T>,
) -> BoxIter<'a, Result<T, Exn>> {
    match pattern {
        Pattern::Var => run(body, (cv.0.cons_var(y), cv.1)),
        Pattern::Index(patterns) => {
            let bound = bind_patterns(patterns, cv.0.clone(), (cv.0, y));
            flat_map_then_with(bound, cv.1, move |ctx, v| run(body, (ctx, v)))
        }
    }
}

fn label_run<'a, T: 'a>(
    cv: (Ctx<'a>, T),
    run: impl FnOnce((Ctx<'a>, T)) -> BoxIter<'a, Result<T, Exn>>,
) -> BoxIter<'a, Result<T, Exn>> {
    let ctx = cv.0.cons_label();
    let label = ctx.labels;
    Box::new(run((ctx, cv.1)).map_while(move |y| match y {
        Err(Exn(Inner::Break(b))) if b == label => None,
        y => Some(y),
    }))
}

fn try_catch_run<'a, T: 'a, I: Iterator<Item = Result<T, Exn>> + 'a>(
    mut ys: BoxIter<'a, Result<T, Exn>>,
    handler: impl Fn(Error) -> I + 'a,
) -> BoxIter<'a, Result<T, Exn>> {
    let mut caught: Option<I> = None;
    Box::new(core::iter::from_fn(move || match &mut caught {
        Some(rest) => rest.next(),
        None => match ys.next()? {
            Err(Exn(Inner::Err(e))) => {
                caught = Some(handler(*e));
                caught.as_mut().and_then(Iterator::next)
            }
            y => Some(y),
        },
    }))
}

enum Step<'a, X, Y> {
    Input(Y),
    Output(X, BoxIter<'a, Result<Y, Exn>>),
}

fn fold<'a, T: 'a, TC: Clone + 'a, U: 'a, UC: 'a>(
    xs: impl Iterator<Item = Result<T, Exn>> + Clone + 'a,
    init: U,
    f: impl Fn(T, U) -> BoxIter<'a, Result<U, Exn>> + 'a,
    keep: impl Fn(&T) -> TC + 'a,
    inner: impl Fn(TC, &U) -> Option<UC> + 'a,
    outer: impl Fn(U) -> Option<UC> + 'a,
) -> impl Iterator<Item = Result<UC, Exn>> + 'a {
    let mut stack = Vec::from([(xs, Step::<TC, U>::Input(init))]);
    core::iter::from_fn(move || {
        loop {
            let (mut xs, step) = stack.pop()?;
            match step {
                Step::Output(x, mut ys) => {
                    let Some(y) = ys.next() else {
                        continue;
                    };
                    if ys.size_hint() != (0, Some(0)) {
                        stack.push((xs.clone(), Step::Output(x.clone(), ys)));
                    }
                    match y {
                        Ok(y) => {
                            let item = inner(x, &y);
                            stack.push((xs, Step::Input(y)));
                            if let Some(item) = item {
                                return Some(Ok(item));
                            }
                        }
                        Err(e) => return Some(Err(e)),
                    }
                }
                Step::Input(y) => match xs.next() {
                    None => {
                        if let Some(item) = outer(y) {
                            return Some(Ok(item));
                        }
                    }
                    Some(Ok(x)) => {
                        let kept = keep(&x);
                        let ys = f(x, y);
                        stack.push((xs, Step::Output(kept, ys)));
                    }
                    Some(Err(e)) => return Some(Err(e)),
                },
            }
        }
    })
}

fn fold_run<'a, T: Clone + 'a>(
    xs: Lazy<'a, Result<Ctx<'a>, Exn>>,
    cv: (Ctx<'a>, T),
    init: TermId,
    update: TermId,
    kind: &'a Fold,
    run: RunFn<'a, T>,
) -> BoxIter<'a, Result<T, Exn>> {
    let init = run(init, cv.clone());
    let update = move |ctx, v| run(update, (ctx, v));
    flat_map_then_with(init, xs, move |i, xs| match kind {
        Fold::Reduce => Box::new(fold(xs, i, update, |_| (), |_, _| None, Some)),
        Fold::Foreach(None) => Box::new(fold(
            xs,
            i,
            update,
            |_| (),
            |_, y: &T| Some(y.clone()),
            |_| None,
        )),
        Fold::Foreach(Some(project)) => {
            let project = *project;
            let states = fold(
                xs,
                i,
                update,
                |ctx: &Ctx<'a>| ctx.clone(),
                |ctx, y: &T| Some((ctx, y.clone())),
                |_| None,
            );
            flat_map_then(states, move |(ctx, y)| run(project, (ctx, y)))
        }
    })
}

fn pipe<'a, T: 'a>(
    l: TermId,
    cv: Cv<'a>,
    r: impl Fn(Cv<'a>, Val) -> BoxIter<'a, Result<T, Exn>> + 'a,
) -> BoxIter<'a, Result<T, Exn>> {
    flat_map_then_with(l.run(cv.clone()), cv, move |y, cv| r(cv, y))
}

fn cartesian<'a>(l: TermId, r: TermId, cv: Cv<'a>) -> BoxIter<'a, (ValX, ValX)> {
    flat_map_with(l.run(cv.clone()), cv, move |l, cv| {
        map_with(r.run(cv), l, |r, l| (l, r))
    })
}

fn fold_update<'a>(
    kind: &'a Fold,
    body: TermId,
    v: Val,
    mut xs: Lazy<'a, Result<Ctx<'a>, Exn>>,
    f: Updater<'a>,
) -> ValXs<'a> {
    let ctx = match xs.next() {
        Some(Ok(ctx)) => ctx,
        Some(Err(e)) => return box_once(Err(e)),
        None => {
            return match kind {
                Fold::Reduce => f(v),
                Fold::Foreach(_) => box_once(Ok(v)),
            };
        }
    };
    let next = move |v, (xs, f)| fold_update(kind, body, v, xs, f);
    let update: Updater<'a> = match kind {
        Fold::Reduce => Rc::new(move |v| next(v, (xs.clone(), f.clone()))),
        Fold::Foreach(None) => {
            Rc::new(move |v| flat_map_then_with(f(v), (xs.clone(), f.clone()), next))
        }
        Fold::Foreach(Some(project)) => {
            let project = *project;
            let ctx = ctx.clone();
            Rc::new(move |v| {
                let projected = project.update((ctx.clone(), v), f.clone());
                flat_map_then_with(projected, (xs.clone(), f.clone()), next)
            })
        }
    };
    body.update((ctx, v), update)
}

fn reduce<'a, T: Clone + 'a>(
    xs: BoxIter<'a, Result<T, Exn>>,
    init: Val,
    f: impl Fn(T, Val) -> ValXs<'a> + 'a,
) -> ValXs<'a> {
    let xs = Lazy::new(xs);
    Box::new(fold(xs, init, f, |_| (), |_, _| None, Some))
}

fn recurse_run<'a, T: Clone + 'a, I: Iterator<Item = Result<T, Error>> + 'a>(
    x: T,
    children: &'a impl Fn(T) -> I,
    depth: usize,
) -> BoxIter<'a, Result<T, Exn>> {
    if depth > EVAL_DEPTH_MAX {
        return box_once(Err(Exn::depth()));
    }
    let first = core::iter::once(Ok(x.clone()));
    Box::new(
        first.chain(
            children(x)
                .flatten()
                .flat_map(move |y| recurse_run(y, children, depth + 1)),
        ),
    )
}

fn recurse_update<'a>(v: Val, f: &Updater<'a>, depth: usize) -> ValXs<'a> {
    if depth > EVAL_DEPTH_MAX {
        return box_once(Err(Exn::depth()));
    }
    then(
        v.map_values(Opt::Optional, |v| recurse_update(v, f, depth + 1)),
        |v| f(v),
    )
}

struct Stack<I, F>(Vec<I>, F);

impl<I: Iterator, F: Fn(I::Item) -> ControlFlow<I::Item, I>> Iterator for Stack<I, F> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        loop {
            let mut top = self.0.pop()?;
            if let Some(next) = top.next() {
                if top.size_hint() != (0, Some(0)) {
                    self.0.push(top);
                }
                match (self.1)(next) {
                    ControlFlow::Break(next) => return Some(next),
                    ControlFlow::Continue(iter) => self.0.push(iter),
                }
            }
        }
    }
}

fn def_run<'a, T: 'a>(
    id: TermId,
    call: CallType,
    cvs: BoxIter<'a, Result<(Ctx<'a>, T), Exn>>,
    run: RunFn<'a, T>,
    site: Ctx<'a>,
    into: fn(T) -> CallInput,
    from: fn(CallInput) -> T,
) -> BoxIter<'a, Result<T, Exn>> {
    let outs =
        |cvs: BoxIter<'a, Result<(Ctx<'a>, T), Exn>>| flat_map_then(cvs, move |cv| run(id, cv));
    let catch = move |all: bool| {
        move |r: Result<T, Exn>| match r {
            Err(Exn(Inner::TailCall(call))) if all || call.0 == id => {
                let (callee, vars, input) = *call;
                ControlFlow::Continue(run(callee, (site.with_vars(vars), from(input))))
            }
            r => ControlFlow::Break(r),
        }
    };
    match call {
        CallType::Inline => outs(cvs),
        CallType::CatchOne => Box::new(Stack(Vec::from([outs(cvs)]), catch(false))),
        CallType::CatchAll => Box::new(Stack(Vec::from([outs(cvs)]), catch(true))),
        CallType::Throw => Box::new(cvs.map(move |cv| {
            cv.and_then(|cv| Err(Exn(Inner::TailCall(Box::new((id, cv.0.vars, into(cv.1)))))))
        })),
    }
}

fn path_err<'a, T: 'a>(v: Val) -> BoxIter<'a, Result<T, Exn>> {
    box_once(Err(path_error(v)))
}

fn project_cv<'a>(cv: &Cvp<'a>) -> Cv<'a> {
    (cv.0.clone(), cv.1.0.clone())
}

fn project_val(vp: &(Val, PathList)) -> Val {
    vp.0.clone()
}

fn native_paths<'a>(cv: Cvp<'a>) -> ValPathXs<'a> {
    let native = &cv.0.data.lut.natives;
    let _ = native;
    path_err(cv.1.0)
}

impl TermId {
    pub fn run<'a>(self, cv: Cv<'a>) -> ValXs<'a> {
        let Some(ctx) = cv.0.enter() else {
            return box_once(Err(Exn::depth()));
        };
        let cv = (ctx, cv.1);
        match cv.0.term(self) {
            Term::Id => box_once(Ok(cv.1)),
            Term::Empty => Box::new(core::iter::empty()),
            Term::Recurse => {
                let depth = cv.0.depth;
                recurse_run(cv.1, &|v: Val| v.values(), depth)
            }
            Term::ToString => box_once(Ok(cv.1.into_string())),
            Term::Literal(v) => box_once(v.clone().map_err(Exn::from)),
            Term::Arr(f) => box_once(f.run(cv).collect::<Result<Vec<Val>, Exn>>().map(Val::arr)),
            Term::ObjEmpty => box_once(Val::from_map([]).map_err(Exn::from)),
            Term::ObjSingle(k, v) => {
                Box::new(cartesian(*k, *v, cv).map(|(k, v)| Ok(Val::from_map([(k?, v?)])?)))
            }
            Term::TryCatch(f, c) => {
                let (ctx, v) = cv;
                let c = *c;
                try_catch_run(f.run((ctx.clone(), v)), move |e| {
                    c.run((ctx.clone(), e.into_val()))
                })
            }
            Term::Neg(f) => Box::new(f.run(cv).map(|v| Ok((-v?)?))),
            Term::Pipe(l, None, r) => {
                let r = *r;
                let (ctx, v) = cv;
                flat_map_then_with(l.run((ctx.clone(), v)), ctx, move |y, ctx| r.run((ctx, y)))
            }
            Term::Pipe(l, Some(pattern), r) => {
                let r = *r;
                pipe(*l, cv, move |cv, y| {
                    bind_run(pattern, r, cv, y, TermId::run)
                })
            }
            Term::Comma(l, r) => {
                let r = *r;
                Box::new(l.run(cv.clone()).chain(lazy(move || r.run(cv))))
            }
            Term::Alt(l, r) => {
                let mut l = l
                    .run(cv.clone())
                    .filter(|v| v.as_ref().map_or(true, Val::as_bool));
                match l.next() {
                    Some(head) => Box::new(core::iter::once(head).chain(l)),
                    None => r.run(cv),
                }
            }
            Term::Ite(condition, then_, else_) => {
                let (then_, else_) = (*then_, *else_);
                pipe(*condition, cv, move |cv, v| {
                    if v.as_bool() { then_ } else { else_ }.run(cv)
                })
            }
            Term::Path(f, path) => {
                let indices = path.map_ref(|i| {
                    let (i, cv) = (*i, cv.clone());
                    collect_if_once(move || i.run(cv))
                });
                flat_map_then_with(f.run(cv), indices, |y, indices| {
                    flat_map_then_with(path::explode(indices), y, |path, y| {
                        Box::new(path.run(y).map(|r| r.map_err(Exn::from)))
                    })
                })
            }
            Term::Update(path, f) => {
                let f = *f;
                let ctx = cv.0.clone();
                path.update(cv, Rc::new(move |v| f.run((ctx.clone(), v))))
            }
            Term::UpdateMath(path, op, f) => {
                let (path, op) = (*path, *op);
                pipe(*f, cv, move |cv, y| {
                    path.update(
                        cv,
                        Rc::new(move |x: Val| box_once(math(op, x, y.clone()).map_err(Exn::from))),
                    )
                })
            }
            Term::UpdateAlt(path, f) => {
                let path = *path;
                pipe(*f, cv, move |cv, y| {
                    path.update(
                        cv,
                        Rc::new(move |x: Val| {
                            box_once(Ok(if x.as_bool() { x } else { y.clone() }))
                        }),
                    )
                })
            }
            Term::Assign(path, f) => {
                let path = *path;
                pipe(*f, cv, move |cv, y| {
                    path.update(cv, Rc::new(move |_| box_once(Ok(y.clone()))))
                })
            }
            Term::Logic(l, stop, r) => {
                let (stop, r) = (*stop, *r);
                pipe(*l, cv, move |cv, l| {
                    if l.as_bool() == stop {
                        box_once(Ok(Val::from(stop)))
                    } else {
                        Box::new(r.run(cv).map(|r| Ok(Val::from(r?.as_bool()))))
                    }
                })
            }
            Term::Math(l, op, r) => {
                let op = *op;
                Box::new(cartesian(*l, *r, cv).map(move |(x, y)| Ok(math(op, x?, y?)?)))
            }
            Term::Cmp(l, op, r) => {
                let op = *op;
                Box::new(
                    cartesian(*l, *r, cv).map(move |(x, y)| Ok(Val::from(compare(op, &x?, &y?)))),
                )
            }
            Term::Fold(source, pattern, init, update, kind) => {
                let xs = Lazy::new(run_and_bind(*source, cv.clone(), pattern));
                fold_run(xs, cv, *init, *update, kind, TermId::run)
            }
            Term::Var(index) => match cv.0.binding(*index) {
                Binding::Var(v) => box_once(Ok(v.clone())),
                Binding::Fun(id, vars) => id.run((cv.0.with_vars(vars.clone()), cv.1)),
                Binding::Label(label) => box_once(Err(Exn(Inner::Break(*label)))),
            },
            Term::CallDef(id, args, skip, call) => {
                let site = cv.0.clone();
                let cvs = bind_vars(args, cv.0.clone().skip_vars(*skip), cv, Clone::clone);
                def_run(
                    *id,
                    *call,
                    cvs,
                    TermId::run,
                    site,
                    CallInput::Run,
                    CallInput::into_run,
                )
            }
            Term::Native(index, args) => {
                let cvs = bind_vars(args, cv.0.with_vars(Vars::default()), cv, Clone::clone);
                let native = *index;
                flat_map_then(cvs, move |cv| (cv.0.data.lut.natives[native].run)(cv))
            }
            Term::Label(f) => {
                let f = *f;
                label_run(cv, move |cv| f.run(cv))
            }
        }
    }

    pub fn paths<'a>(self, cv: Cvp<'a>) -> ValPathXs<'a> {
        let Some(ctx) = cv.0.enter() else {
            return box_once(Err(Exn::depth()));
        };
        let cv = (ctx, cv.1);
        match cv.0.term(self) {
            Term::ToString | Term::Literal(_) | Term::Empty => path_err(cv.1.0),
            Term::Arr(_) | Term::ObjEmpty | Term::ObjSingle(..) => path_err(cv.1.0),
            Term::Neg(_) | Term::Logic(..) | Term::Math(..) | Term::Cmp(..) => path_err(cv.1.0),
            Term::Update(..) | Term::Assign(..) | Term::UpdateMath(..) | Term::UpdateAlt(..) => {
                path_err(cv.1.0)
            }
            Term::Id => box_once(Ok(cv.1)),
            Term::Recurse => {
                let depth = cv.0.depth;
                recurse_run(
                    cv.1,
                    &|(v, p): (Val, PathList)| {
                        v.key_values()
                            .map(move |r| r.map(|(k, child)| (child, p.clone().cons(k))))
                    },
                    depth,
                )
            }
            Term::Pipe(l, None, r) => {
                let r = *r;
                let (ctx, vp) = cv;
                flat_map_then_with(l.paths((ctx.clone(), vp)), ctx, move |y, ctx| {
                    r.paths((ctx, y))
                })
            }
            Term::Pipe(l, Some(pattern), r) => {
                let r = *r;
                flat_map_then_with(l.run(project_cv(&cv)), cv, move |y, cv| {
                    bind_run(pattern, r, cv, y, TermId::paths)
                })
            }
            Term::Comma(l, r) => {
                let r = *r;
                Box::new(l.paths(cv.clone()).chain(lazy(move || r.paths(cv))))
            }
            Term::Alt(l, r) => {
                let any_true = l
                    .run(project_cv(&cv))
                    .any(|v| v.as_ref().map_or(true, Val::as_bool));
                if any_true { *l } else { *r }.paths(cv)
            }
            Term::Ite(condition, then_, else_) => {
                let (then_, else_) = (*then_, *else_);
                flat_map_then_with(condition.run(project_cv(&cv)), cv, move |v, cv| {
                    if v.as_bool() { then_ } else { else_ }.paths(cv)
                })
            }
            Term::TryCatch(f, c) => {
                let c = *c;
                let (ctx, vp) = cv;
                try_catch_run(f.paths((ctx.clone(), vp)), move |e| {
                    c.run((ctx.clone(), e.into_val()))
                        .map(|e| Err(path_error(e?)))
                })
            }
            Term::Path(f, path) => {
                let indices = path.map_ref(|i| {
                    let (i, cv) = (*i, project_cv(&cv));
                    collect_if_once(move || i.run(cv))
                });
                flat_map_then_with(f.paths(cv), indices, |y, indices| {
                    flat_map_then_with(path::explode(indices), y, |path, y| {
                        Box::new(path.paths(y).map(|r| r.map_err(Exn::from)))
                    })
                })
            }
            Term::Var(index) => match cv.0.binding(*index) {
                Binding::Var(_) => path_err(cv.1.0),
                Binding::Fun(id, vars) => id.paths((cv.0.with_vars(vars.clone()), cv.1)),
                Binding::Label(label) => box_once(Err(Exn(Inner::Break(*label)))),
            },
            Term::Fold(source, pattern, init, update, kind) => {
                let xs = Lazy::new(run_and_bind(*source, project_cv(&cv), pattern));
                fold_run(xs, cv, *init, *update, kind, TermId::paths)
            }
            Term::CallDef(id, args, skip, call) => {
                let site = cv.0.clone();
                let cvs = bind_vars(args, cv.0.clone().skip_vars(*skip), cv, project_val);
                def_run(
                    *id,
                    *call,
                    cvs,
                    TermId::paths,
                    site,
                    CallInput::Paths,
                    CallInput::into_paths,
                )
            }
            Term::Label(f) => {
                let f = *f;
                label_run(cv, move |cv| f.paths(cv))
            }
            Term::Native(index, args) => {
                let cvs = bind_vars(args, cv.0.with_vars(Vars::default()), cv, project_val);
                let native = *index;
                flat_map_then(cvs, move |cv| match cv.0.data.lut.natives[native].paths {
                    Some(paths) => paths(cv),
                    None => native_paths(cv),
                })
            }
        }
    }

    pub fn update<'a>(self, cv: Cv<'a>, f: Updater<'a>) -> ValXs<'a> {
        let Some(ctx) = cv.0.enter() else {
            return box_once(Err(Exn::depth()));
        };
        let cv = (ctx, cv.1);
        match cv.0.term(self) {
            Term::ToString | Term::Literal(_) | Term::Empty => path_err(cv.1),
            Term::Arr(_) | Term::ObjEmpty | Term::ObjSingle(..) => path_err(cv.1),
            Term::Neg(_) | Term::Logic(..) | Term::Math(..) | Term::Cmp(..) => path_err(cv.1),
            Term::Update(..) | Term::Assign(..) | Term::UpdateMath(..) | Term::UpdateAlt(..) => {
                path_err(cv.1)
            }
            Term::TryCatch(..) | Term::Label(..) => path_err(cv.1),
            Term::Id => f(cv.1),
            Term::Recurse => {
                let depth = cv.0.depth;
                recurse_update(cv.1, &f, depth)
            }
            Term::Path(l, path) => {
                let indices = path.map_ref(|i| {
                    let (i, cv) = (*i, cv.clone());
                    collect_if_once(move || i.run(cv))
                });
                let each = move |v: Val| {
                    let mut paths = path::explode(indices.clone());
                    box_once(paths.try_fold(v, |acc, path| path?.update(acc, |v| f(v))))
                };
                l.update(cv, Rc::new(each))
            }
            Term::Pipe(l, None, r) => {
                let r = *r;
                let ctx = cv.0.clone();
                l.update(cv, Rc::new(move |v| r.update((ctx.clone(), v), f.clone())))
            }
            Term::Pipe(l, Some(pattern), r) => {
                let r = *r;
                let bound = run_and_bind(*l, (cv.0, cv.1.clone()), pattern);
                reduce(bound, cv.1, move |ctx, v| r.update((ctx, v), f.clone()))
            }
            Term::Comma(l, r) => {
                let r = *r;
                let (ctx, v) = cv;
                flat_map_then_with(
                    l.update((ctx.clone(), v), f.clone()),
                    (ctx, f),
                    move |v, (ctx, f)| r.update((ctx, v), f),
                )
            }
            Term::Ite(condition, then_, else_) => {
                let (then_, else_) = (*then_, *else_);
                let ctx = cv.0.clone();
                reduce(condition.run(cv.clone()), cv.1, move |x, v| {
                    if x.as_bool() { then_ } else { else_ }.update((ctx.clone(), v), f.clone())
                })
            }
            Term::Alt(l, r) => {
                let some_true = l.run(cv.clone()).any(|y| y.map_or(true, |y| y.as_bool()));
                if some_true { *l } else { *r }.update(cv, f)
            }
            Term::Fold(source, pattern, init, update, kind) => {
                let xs = Lazy::new(run_and_bind(*source, cv.clone(), pattern));
                let update = *update;
                let each = move |v| fold_update(kind, update, v, xs.clone(), f.clone());
                init.update(cv, Rc::new(each))
            }
            Term::Var(index) => match cv.0.binding(*index) {
                Binding::Var(_) => path_err(cv.1),
                Binding::Fun(id, vars) => id.update((cv.0.with_vars(vars.clone()), cv.1), f),
                Binding::Label(label) => box_once(Err(Exn(Inner::Break(*label)))),
            },
            Term::CallDef(id, args, skip, _) => {
                let id = *id;
                let init = cv.1.clone();
                let cvs = bind_vars(args, cv.0.clone().skip_vars(*skip), cv, Clone::clone);
                reduce(cvs, init, move |cv: Cv<'a>, v| {
                    id.update((cv.0, v), f.clone())
                })
            }
            Term::Native(_, args) => {
                let init = cv.1.clone();
                let cvs = bind_vars(args, cv.0.with_vars(Vars::default()), cv, Clone::clone);
                reduce(cvs, init, |_: Cv<'a>, v| path_err(v))
            }
        }
    }
}

fn math(op: Math, l: Val, r: Val) -> Result<Val, Error> {
    match op {
        Math::Add => l + r,
        Math::Sub => l - r,
        Math::Mul => l * r,
        Math::Div => l / r,
        Math::Rem => l % r,
    }
}

fn compare(op: Cmp, l: &Val, r: &Val) -> bool {
    match op {
        Cmp::Gt => l > r,
        Cmp::Ge => l >= r,
        Cmp::Lt => l < r,
        Cmp::Le => l <= r,
        Cmp::Eq => l == r,
        Cmp::Ne => l != r,
    }
}

pub fn v(count: usize) -> Box<[Bind]> {
    core::iter::repeat_n(Bind::Var(()), count).collect()
}

pub fn bome<'a>(r: Result<Val, Error>) -> ValXs<'a> {
    box_once(r.map_err(Exn::from))
}

pub fn unary<'a>(mut cv: Cv<'a>, f: impl Fn(Val, Val) -> Result<Val, Error> + 'a) -> ValXs<'a> {
    let arg = cv.0.pop_var();
    bome(f(cv.1, arg))
}

fn range(mut from: ValX, to: Val, by: Val) -> impl Iterator<Item = ValX> {
    let direction = by
        .partial_cmp(&Val::from(0isize))
        .unwrap_or(Ordering::Equal);
    core::iter::from_fn(move || match from.clone() {
        Ok(x) => {
            let more = match direction {
                Ordering::Greater => x < to,
                Ordering::Less => x > to,
                Ordering::Equal => x != to,
            };
            more.then(|| core::mem::replace(&mut from, (x + by.clone()).map_err(Exn::from)))
        }
        e @ Err(_) => {
            from = Ok(to.clone());
            Some(e)
        }
    })
}

fn limited<'a, T: 'a>(
    n: Val,
    mut iter: BoxIter<'a, Result<T, Exn>>,
) -> BoxIter<'a, Result<T, Exn>> {
    if n <= Val::from(0isize) {
        return Box::new(core::iter::empty());
    }
    let mut left = Some(n);
    Box::new(core::iter::from_fn(move || {
        if let Some(i) = left.take().filter(|i| *i > Val::from(0isize)) {
            match i - Val::from(1isize) {
                Ok(i) => left = Some(i),
                Err(e) => return Some(Err(e.into())),
            }
            return iter.next();
        }
        None
    }))
}

fn path_of(path: PathList) -> Val {
    let mut parts: Vec<Val> = path.iter().cloned().collect();
    parts.reverse();
    Val::arr(parts)
}

pub fn core_natives() -> Vec<NativeDef> {
    let fun = || -> Box<[Bind]> { Box::new([Bind::Fun(())]) };
    let var_fun = || -> Box<[Bind]> { Box::new([Bind::Var(()), Bind::Fun(())]) };
    Vec::from([
        (
            "path",
            fun(),
            Native {
                run: |mut cv| {
                    let (f, closure) = cv.0.pop_fun();
                    Box::new(
                        f.paths((closure, (cv.1, PathList::default())))
                            .map(|vp| Ok(path_of(vp?.1))),
                    )
                },
                paths: None,
            },
        ),
        (
            "_range3",
            v(3),
            Native {
                run: |mut cv| {
                    let by = cv.0.pop_var();
                    let to = cv.0.pop_var();
                    let from = cv.0.pop_var();
                    Box::new(range(Ok(from), to, by))
                },
                paths: None,
            },
        ),
        (
            "first",
            fun(),
            Native {
                run: |mut cv| {
                    let (f, closure) = cv.0.pop_fun();
                    Box::new(f.run((closure, cv.1)).next().into_iter())
                },
                paths: Some(|mut cv| {
                    let (f, closure) = cv.0.pop_fun();
                    Box::new(f.paths((closure, cv.1)).next().into_iter())
                }),
            },
        ),
        (
            "_limit",
            var_fun(),
            Native {
                run: |mut cv| {
                    let (f, closure) = cv.0.pop_fun();
                    let n = cv.0.pop_var();
                    if n <= Val::from(0isize) {
                        return Box::new(core::iter::empty());
                    }
                    limited(n, f.run((closure, cv.1)))
                },
                paths: Some(|mut cv| {
                    let (f, closure) = cv.0.pop_fun();
                    let n = cv.0.pop_var();
                    if n <= Val::from(0isize) {
                        return Box::new(core::iter::empty());
                    }
                    limited(n, f.paths((closure, cv.1)))
                }),
            },
        ),
    ])
}
