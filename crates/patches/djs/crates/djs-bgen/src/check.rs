//! The subset checker: names, types, the numeric rules, tags and control flow.
//!
//! Numeric rules (both backends must agree, so operations whose JS and Rust meanings
//! differ are rejected):
//! - `+ - *` on one integer type (i32 or u32) give that type. Overflow is a bug in the
//!   builtin: the Rust backend checks it, the JS backend does not (D1171).
//! - `/` and `%` only on f64 (JS integer division is not Rust's).
//! - `& | ^ ~ << >>` only on i32 (JS converts to int32), `>>>` on i32 gives u32.
//! - Mixed numeric types are rejected, except that i32/u32 widen to f64 implicitly
//!   where a value is assigned or passed (an exact conversion). Integer literals adapt
//!   to the type they meet if they fit.
//! - Comparisons need operands of one numeric type; `===`/`!==` need related types.

use std::collections::HashMap;

use crate::ast::*;
use crate::diag::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq)]
pub struct Sig {
    pub this_ty: Option<Type>,
    /// (type, optional)
    pub params: Vec<(Type, bool)>,
    pub rest: Option<Type>,
    pub ret: Type,
}

impl Sig {
    fn min_args(&self) -> usize {
        self.params.iter().filter(|(_, opt)| !opt).count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callee {
    Intrinsic {
        reentrant: bool,
    },
    /// Index into `Program::builtins`.
    Builtin(usize),
}

/// How a builtin is installed (D1172, D1310).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// `@builtin`: a method (writable, configurable, not enumerable, unless
    /// `@attributes` says otherwise).
    Method {
        attrs: Option<u32>,
    },
    /// `@constructor Name` (+ `@extends Parent`): a global constructor with its
    /// prototype object.
    Constructor {
        parent: Option<String>,
    },
    /// `@getter` / `@setter`: one half of an accessor (configurable, not enumerable).
    Getter,
    Setter,
    /// `@value` (+ `@attributes`): a data property whose value is the result of calling
    /// the function once at install time. `attrs` are PropFlags bits (writable 1,
    /// enumerable 2, configurable 4).
    Value {
        attrs: u32,
    },
}

/// Where a builtin is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// Dotted path from the global object, e.g. `Array.prototype`; `globalThis` is the
    /// global object itself.
    pub target: String,
    pub key: String,
    /// The function's `length`.
    pub length: u32,
    pub kind: InstallKind,
}

impl Install {
    /// Installs onto the global object run first, so that later paths resolve.
    pub fn on_global(&self) -> bool {
        self.target == "globalThis"
    }

    /// For comments and the report: `Array.prototype.map`, `get Symbol.prototype.description`,
    /// `constructor Error`, `value Math.PI`.
    pub fn label(&self) -> String {
        let path = if self.on_global() {
            self.key.clone()
        } else if self.key.starts_with('[') {
            format!("{}{}", self.target, self.key)
        } else {
            format!("{}.{}", self.target, self.key)
        };
        match self.kind {
            InstallKind::Method { .. } => path,
            InstallKind::Constructor { .. } => format!("constructor {path}"),
            InstallKind::Getter => format!("get {path}"),
            InstallKind::Setter => format!("set {path}"),
            InstallKind::Value { .. } => format!("value {path}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Builtin {
    pub module: usize,
    pub index: usize,
    pub name: String,
    /// Where it is installed: none, one, or several places (`Number.parseInt` is the
    /// global `parseInt`).
    pub installs: Vec<Install>,
    /// `@intrinsic`: registered so that privileged code can call it as `%Name(...)`.
    pub intrinsic: bool,
    /// `@native`: also compiled by the Rust backend (later).
    pub native: bool,
    /// `@fast`: has a leaf fast path in Rust (`builtins/fast.rs`, D1850), tried before the
    /// builtin runs; the prelude marks it with `%MarkFast(f)`.
    pub fast: bool,
    /// `@slim-omit`: left out of the slim build's prelude, like every builtin of a file
    /// marked `// @slim-omit` (D2411).
    pub slim_omit: bool,
    /// `@frame`: keeps a frame of its own, never inlined into its callers (0230): the
    /// engine looks for it on the stack.
    pub frame: bool,
}

/// A checked program: every name resolved and every function type-correct.
pub struct Program {
    pub decls: Declarations,
    pub modules: Vec<Module>,
    pub builtins: Vec<Builtin>,
    pub callees: HashMap<String, (Callee, Sig)>,
}

impl Program {
    pub fn function(&self, b: &Builtin) -> &Function {
        &self.modules[b.module].functions[b.index]
    }
}

const INTRINSIC_TAGS: &[&str] = &["leaf", "reentrant"];
const BUILTIN_TAGS: &[&str] = &[
    "builtin",
    "constructor",
    "extends",
    "getter",
    "setter",
    "value",
    "attributes",
    "length",
    "native",
    "intrinsic",
    "fast",
    "slim-omit",
    "frame",
];

pub fn check(decls: Declarations, modules: Vec<Module>) -> Result<Program, Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let mut callees: HashMap<String, (Callee, Sig)> = HashMap::new();
    let mut spans: HashMap<String, Span> = HashMap::new();

    for d in &decls.intrinsics {
        let mut kind = None;
        for t in &d.tags {
            if !INTRINSIC_TAGS.contains(&t.name.as_str()) {
                errors.push(Diagnostic::new(
                    d.span,
                    format!("unknown tag `@{}` on an intrinsic", t.name),
                ));
            } else if kind.is_some() {
                errors.push(Diagnostic::new(
                    d.span,
                    "an intrinsic has exactly one of `@leaf` and `@reentrant`",
                ));
            } else {
                kind = Some(t.name == "reentrant");
            }
        }
        let Some(reentrant) = kind else {
            errors.push(Diagnostic::new(
                d.span,
                format!("intrinsic `{}` needs `@leaf` or `@reentrant`", d.name),
            ));
            continue;
        };
        let sig = Sig {
            this_ty: None,
            params: d
                .params
                .iter()
                .filter(|p| !p.rest)
                .map(|p| (p.ty.ty, p.optional))
                .collect(),
            rest: d.params.iter().find(|p| p.rest).map(|p| p.ty.ty),
            ret: d.ret.ty,
        };
        if spans.insert(d.name.clone(), d.span).is_some() {
            errors.push(Diagnostic::new(
                d.span,
                format!("`{}` is declared twice", d.name),
            ));
        }
        callees.insert(d.name.clone(), (Callee::Intrinsic { reentrant }, sig));
    }

    let mut builtins = Vec::new();
    let mut installs: HashMap<(String, String, String), Span> = HashMap::new();
    for (mi, m) in modules.iter().enumerate() {
        for (fi, f) in m.functions.iter().enumerate() {
            if spans.insert(f.name.clone(), f.name_span).is_some() {
                errors.push(Diagnostic::new(
                    f.name_span,
                    format!("`{}` is already declared", f.name),
                ));
                continue;
            }
            let b = builtin_info(mi, fi, f, &mut errors);
            for i in &b.installs {
                // A getter and a setter share their key.
                let half = match i.kind {
                    InstallKind::Getter => "get",
                    InstallKind::Setter => "set",
                    _ => "",
                };
                if installs
                    .insert(
                        (i.target.clone(), i.key.clone(), half.to_string()),
                        f.name_span,
                    )
                    .is_some()
                {
                    errors.push(Diagnostic::new(
                        f.name_span,
                        format!("{}.{} is installed twice", i.target, i.key),
                    ));
                }
            }
            let sig = Sig {
                this_ty: f.this_ty.as_ref().map(|t| t.ty),
                params: f
                    .params
                    .iter()
                    .filter(|p| !p.rest)
                    .map(|p| (p.ty.ty, p.optional))
                    .collect(),
                rest: f.params.iter().find(|p| p.rest).map(|p| p.ty.ty),
                ret: f.ret.ty,
            };
            callees.insert(f.name.clone(), (Callee::Builtin(builtins.len()), sig));
            builtins.push(b);
        }
    }

    for m in &modules {
        for f in &m.functions {
            FnChecker::new(&callees, f, &mut errors).check();
        }
    }
    if errors.is_empty() {
        Ok(Program {
            decls,
            modules,
            builtins,
            callees,
        })
    } else {
        Err(errors)
    }
}

/// Splits `A.b.c` into (`A.b`, `c`); a bare name `X` is (`globalThis`, `X`). The last
/// component may be a well-known symbol: `A.prototype[Symbol.iterator]` gives the key
/// `[Symbol.iterator]`.
fn split_path(path: &str) -> Option<(String, String)> {
    let ident = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    };
    // A target is a dotted path; its first segment may name a realm intrinsic object
    // (`%ArrayIteratorPrototype%`), which `%InstallMethod` resolves.
    let target_ok = |t: &str| {
        t.split('.').enumerate().all(|(i, seg)| {
            ident(seg)
                || (i == 0
                    && seg.len() > 2
                    && seg.starts_with('%')
                    && seg.ends_with('%')
                    && ident(&seg[1..seg.len() - 1]))
        })
    };
    if let Some(open) = path.find('[') {
        let (target, key) = path.split_at(open);
        let name = key.strip_prefix("[Symbol.")?.strip_suffix(']')?;
        if !ident(name) || !target_ok(target) {
            return None;
        }
        return Some((target.into(), key.into()));
    }
    // A bare `%Name%` (only for `@constructor %TypedArray%`): a constructor that is a
    // realm intrinsic instead of a global (D1391).
    if path.len() > 2
        && path.starts_with('%')
        && path.ends_with('%')
        && ident(&path[1..path.len() - 1])
    {
        return Some(("globalThis".into(), path.into()));
    }
    let (t, k) = path.rsplit_once('.').unwrap_or(("", path));
    if !ident(k) || (!t.is_empty() && !target_ok(t)) {
        return None;
    }
    Some(match path.rsplit_once('.') {
        Some((t, k)) => (t.into(), k.into()),
        None => ("globalThis".into(), path.into()),
    })
}

/// The well-known symbol name of a `[Symbol.name]` key.
pub fn symbol_key(key: &str) -> Option<&str> {
    key.strip_prefix("[Symbol.")?.strip_suffix(']')
}

fn builtin_info(
    module: usize,
    index: usize,
    f: &Function,
    errors: &mut Vec<Diagnostic>,
) -> Builtin {
    let mut b = Builtin {
        module,
        index,
        name: f.name.clone(),
        installs: Vec::new(),
        intrinsic: false,
        native: false,
        fast: false,
        slim_omit: false,
        frame: false,
    };
    let mut length = None;
    let mut parent = None;
    let mut attrs = None;
    let mut err = |msg: String| errors.push(Diagnostic::new(f.name_span, msg));
    for t in &f.tags {
        let kind = match t.name.as_str() {
            "builtin" => Some(InstallKind::Method { attrs: None }),
            "constructor" => Some(InstallKind::Constructor { parent: None }),
            "getter" => Some(InstallKind::Getter),
            "setter" => Some(InstallKind::Setter),
            "value" => Some(InstallKind::Value { attrs: 0 }),
            _ => None,
        };
        if let Some(kind) = kind {
            let Some((target, key)) = split_path(&t.value) else {
                err(format!(
                    "`@{} {}`: expected a dotted path such as Array.prototype.map",
                    t.name, t.value
                ));
                continue;
            };
            if matches!(kind, InstallKind::Constructor { .. })
                && (target != "globalThis" || symbol_key(&key).is_some())
            {
                err(format!(
                    "`@constructor {}`: constructors are globals; use a plain name",
                    t.value
                ));
            }
            b.installs.push(Install {
                target,
                key,
                length: 0,
                kind,
            });
            continue;
        }
        match t.name.as_str() {
            "length" => match t.value.parse::<u32>() {
                Ok(n) => length = Some(n),
                Err(_) => err(format!("`@length {}`: expected a number", t.value)),
            },
            "extends" => match split_path(&t.value) {
                Some(_) => parent = Some(t.value.clone()),
                None => err(format!("`@extends {}`: expected a path", t.value)),
            },
            "attributes" => {
                let mut a = 0;
                for w in t.value.split_whitespace() {
                    a |= match w {
                        "writable" => 1,
                        "enumerable" => 2,
                        "configurable" => 4,
                        // Explicitly none (for `@builtin`, whose default is
                        // writable configurable).
                        "none" => 0,
                        _ => {
                            err(format!(
                                "`@attributes`: unknown attribute `{w}` (writable, enumerable, configurable, none)"
                            ));
                            0
                        }
                    };
                }
                attrs = Some(a);
            }
            "native" => b.native = true,
            "fast" => b.fast = true,
            "slim-omit" => b.slim_omit = true,
            "frame" => b.frame = true,
            "intrinsic" => b.intrinsic = true,
            other => err(format!(
                "unknown tag `@{other}` (builtin functions know {})",
                BUILTIN_TAGS
                    .iter()
                    .map(|t| format!("@{t}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    let kinds =
        |pred: fn(&InstallKind) -> bool| b.installs.iter().filter(|i| pred(&i.kind)).count();
    let ctors = kinds(|k| matches!(k, InstallKind::Constructor { .. }));
    let values = kinds(|k| matches!(k, InstallKind::Value { .. }));
    let accessors = kinds(|k| matches!(k, InstallKind::Getter | InstallKind::Setter));
    // One function may be several accessor halves (%ThrowTypeError%), but not mix them
    // with other forms.
    let mixed_accessors = accessors > 0 && accessors < b.installs.len();
    if (ctors > 0 || values > 0 || mixed_accessors) && b.installs.len() > 1 {
        err(
            "`@constructor` and `@value` install a function once, alone; `@getter`/`@setter` mix only with each other"
                .into(),
        );
    }
    if parent.is_some() && ctors == 0 {
        err("`@extends` needs `@constructor`".into());
    }
    let methods = kinds(|k| matches!(k, InstallKind::Method { .. }));
    if attrs.is_some() && values == 0 && methods == 0 {
        err("`@attributes` needs `@value` or `@builtin`".into());
    }
    if length.is_some() && (b.installs.is_empty() || values > 0) {
        err("`@length` needs `@builtin`, `@constructor`, `@getter` or `@setter`".into());
    }
    if values > 0 && (f.this_ty.is_some() || !f.params.is_empty()) {
        err("a `@value` function takes no parameters".into());
    }
    // The spec's `length`: the parameters before the first optional one (or the rest).
    let default_length = f
        .params
        .iter()
        .take_while(|p| !p.optional && !p.rest)
        .count() as u32;
    for i in &mut b.installs {
        i.length = length.unwrap_or(default_length);
        match &mut i.kind {
            InstallKind::Constructor { parent: p } => *p = parent.clone(),
            InstallKind::Value { attrs: a } => *a = attrs.unwrap_or(0),
            InstallKind::Method { attrs: a } => *a = attrs,
            _ => {}
        }
    }
    b
}

struct Local {
    ty: Type,
    mutable: bool,
}

struct FnChecker<'a> {
    callees: &'a HashMap<String, (Callee, Sig)>,
    f: &'a Function,
    errors: &'a mut Vec<Diagnostic>,
    scopes: Vec<HashMap<String, Local>>,
    loops: usize,
}

/// Whether `from` can be used where `to` is expected.
pub fn assignable(from: Type, to: Type) -> bool {
    if from == to || from == Type::Never || from == Type::Error || to == Type::Error {
        return true;
    }
    match (from, to) {
        (Type::Void, _) => false,
        (_, Type::Value) => true,
        (Type::Fn, Type::Obj) => true,
        (Type::IntLit(v), Type::I32) => v >= i32::MIN as f64 && v <= i32::MAX as f64,
        (Type::IntLit(v), Type::U32) => v >= 0.0 && v <= u32::MAX as f64,
        (Type::IntLit(_) | Type::NumLit | Type::I32 | Type::U32, Type::F64) => true,
        _ => false,
    }
}

pub fn literal_type(v: f64) -> Type {
    if v.fract() == 0.0 && v.abs() <= 9007199254740991.0 {
        Type::IntLit(v)
    } else {
        Type::NumLit
    }
}

impl<'a> FnChecker<'a> {
    fn new(
        callees: &'a HashMap<String, (Callee, Sig)>,
        f: &'a Function,
        errors: &'a mut Vec<Diagnostic>,
    ) -> Self {
        FnChecker {
            callees,
            f,
            errors,
            scopes: Vec::new(),
            loops: 0,
        }
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.errors.push(Diagnostic::new(span, msg));
    }

    fn check(mut self) {
        let mut params = HashMap::new();
        for p in &self.f.params {
            if params
                .insert(
                    p.name.clone(),
                    Local {
                        // `...args: Value[]` is an array object.
                        ty: if p.rest { Type::Obj } else { p.ty.ty },
                        mutable: true,
                    },
                )
                .is_some()
            {
                self.error(p.span, format!("duplicate parameter `{}`", p.name));
            }
            if matches!(p.ty.ty, Type::Void | Type::Never) {
                self.error(p.ty.span, "a parameter cannot be void or never");
            }
        }
        self.scopes.push(params);
        let terminates = self.block(&self.f.body);
        let ret = self.f.ret.ty;
        if !terminates && ret != Type::Void {
            self.error(
                self.f.name_span,
                format!(
                    "`{}` can reach its end without returning a {}",
                    self.f.name,
                    ret.name()
                ),
            );
        }
    }

    fn lookup(&self, name: &str) -> Option<&Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn declare(&mut self, name: &str, ty: Type, mutable: bool, span: Span) {
        if self.lookup(name).is_some() {
            self.error(
                span,
                format!("`{name}` shadows another local; use a different name"),
            );
        }
        if self.callees.contains_key(name) {
            self.error(span, format!("`{name}` shadows a function"));
        }
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), Local { ty, mutable });
    }

    /// Returns true if the block never completes normally (returns, throws, loops).
    fn block(&mut self, b: &Block) -> bool {
        self.scopes.push(HashMap::new());
        let mut terminates = false;
        for s in &b.stmts {
            terminates |= self.stmt(s);
        }
        self.scopes.pop();
        terminates
    }

    fn sub_stmt(&mut self, s: &Stmt) -> bool {
        // A declaration as the whole body of an if/loop would leak nowhere; reject it.
        if let Stmt::Decl { span, .. } = s {
            self.error(*span, "a declaration must be inside a block");
        }
        self.scopes.push(HashMap::new());
        let t = self.stmt(s);
        self.scopes.pop();
        t
    }

    fn expect_type(&mut self, e: &Expr, want: Type, what: &str) {
        let got = self.expr(e);
        if !assignable(got, want) {
            self.error(
                e.span,
                format!("{what}: expected {}, found {}", want.name(), got.name()),
            );
        }
    }

    fn stmt(&mut self, s: &Stmt) -> bool {
        match s {
            Stmt::Decl {
                kind,
                name,
                ty,
                init,
                span,
            } => {
                if matches!(ty.ty, Type::Void | Type::Never) {
                    self.error(ty.span, "a variable cannot be void or never");
                }
                self.expect_type(init, ty.ty, &format!("initializer of `{name}`"));
                self.declare(name, ty.ty, *kind == DeclKind::Let, *span);
                false
            }
            Stmt::Assign {
                target,
                target_span,
                op,
                value,
                ..
            } => {
                let Some(local) = self.lookup(target) else {
                    self.error(*target_span, format!("unknown local `{target}`"));
                    return false;
                };
                let (ty, mutable) = (local.ty, local.mutable);
                if !mutable {
                    self.error(*target_span, format!("`{target}` is a const"));
                }
                match op.binary() {
                    None => self.expect_type(value, ty, &format!("assignment to `{target}`")),
                    Some(bin) => {
                        let v = self.expr(value);
                        if let Some(result) = self.binary_type(bin, ty, v, *target_span, value.span)
                            && !assignable(result, ty)
                        {
                            self.error(
                                value.span,
                                format!(
                                    "`{}` on {} gives {}",
                                    op.as_str(),
                                    ty.name(),
                                    result.name()
                                ),
                            );
                        }
                    }
                }
                false
            }
            Stmt::Update { target, span, .. } => {
                match self.lookup(target) {
                    None => self.error(*span, format!("unknown local `{target}`")),
                    Some(l) if !l.mutable => self.error(*span, format!("`{target}` is a const")),
                    Some(l) if !matches!(l.ty, Type::I32 | Type::U32 | Type::F64) => {
                        let msg = format!(
                            "`++`/`--` need a numeric local, `{target}` is {}",
                            l.ty.name()
                        );
                        self.error(*span, msg)
                    }
                    Some(_) => {}
                }
                false
            }
            Stmt::Expr(e) => {
                let ExprKind::Call { .. } = &e.kind else {
                    self.error(e.span, "an expression statement must be a call");
                    return false;
                };
                self.expr(e) == Type::Never
            }
            Stmt::If {
                cond, then, els, ..
            } => {
                self.expect_type(cond, Type::Bool, "condition");
                let t = self.sub_stmt(then);
                let e = els.as_ref().map(|s| self.sub_stmt(s)).unwrap_or(false);
                t && e
            }
            Stmt::While { cond, body, .. } => {
                self.expect_type(cond, Type::Bool, "condition");
                self.loops += 1;
                self.sub_stmt(body);
                self.loops -= 1;
                is_true(cond) && !has_break(body)
            }
            Stmt::DoWhile { body, cond, .. } => {
                self.loops += 1;
                let t = self.sub_stmt(body);
                self.loops -= 1;
                self.expect_type(cond, Type::Bool, "condition");
                (t || is_true(cond)) && !has_break(body)
            }
            Stmt::For {
                init,
                cond,
                update,
                body,
                ..
            } => {
                self.scopes.push(HashMap::new());
                if let Some(i) = init {
                    match &**i {
                        Stmt::Decl { .. } | Stmt::Assign { .. } | Stmt::Update { .. } => {
                            self.stmt(i);
                        }
                        other => self.error(
                            crate::parser::stmt_span(other),
                            "a for initializer is a declaration or an assignment",
                        ),
                    }
                }
                if let Some(c) = cond {
                    self.expect_type(c, Type::Bool, "condition");
                }
                if let Some(u) = update {
                    match &**u {
                        Stmt::Assign { .. } | Stmt::Update { .. } => {
                            self.stmt(u);
                        }
                        other => self.error(
                            crate::parser::stmt_span(other),
                            "a for update is an assignment or `++`/`--`",
                        ),
                    }
                }
                self.loops += 1;
                self.sub_stmt(body);
                self.loops -= 1;
                self.scopes.pop();
                cond.is_none() && !has_break(body)
            }
            Stmt::Return { value, span } => {
                let ret = self.f.ret.ty;
                match value {
                    None if ret != Type::Void => {
                        self.error(*span, format!("`return` needs a {} value", ret.name()))
                    }
                    Some(v) if ret == Type::Void => {
                        self.error(v.span, "a void function returns no value")
                    }
                    Some(v) => self.expect_type(v, ret, "return value"),
                    None => {}
                }
                true
            }
            Stmt::Throw { value, .. } => {
                self.expect_type(value, Type::Value, "thrown value");
                true
            }
            Stmt::Try {
                body,
                catch,
                finally,
                ..
            } => {
                let t = self.block(body);
                let c = match catch {
                    Some(c) => {
                        self.scopes.push(HashMap::new());
                        if let Some((name, span)) = &c.name {
                            self.declare(name, Type::Value, false, *span);
                        }
                        let r = self.block(&c.body);
                        self.scopes.pop();
                        r
                    }
                    None => true,
                };
                let f = finally.as_ref().map(|b| self.block(b)).unwrap_or(false);
                (t && c) || f
            }
            Stmt::Break(span) | Stmt::Continue(span) => {
                if self.loops == 0 {
                    self.error(*span, "`break`/`continue` outside a loop");
                }
                true
            }
            Stmt::Block(b) => self.block(b),
        }
    }

    fn expr(&mut self, e: &Expr) -> Type {
        match &e.kind {
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(l) => l.ty,
                None if self.callees.contains_key(name) => {
                    self.error(
                        e.span,
                        format!("`{name}` is a function; functions are not values in the subset"),
                    );
                    Type::Value
                }
                None => {
                    self.error(e.span, format!("unknown name `{name}`"));
                    Type::Error
                }
            },
            ExprKind::This => match &self.f.this_ty {
                Some(t) => t.ty,
                None => {
                    self.error(e.span, "`this` needs a `this: T` parameter");
                    Type::Error
                }
            },
            ExprKind::NewTarget => {
                // `@intrinsic` too: Rust may install one as a constructor (the
                // function-kind constructors, D1420, 0059).
                if !self
                    .f
                    .tags
                    .iter()
                    .any(|t| t.name == "constructor" || t.name == "intrinsic")
                {
                    self.error(
                        e.span,
                        "`new.target` is only available in `@constructor` and `@intrinsic` builtins",
                    );
                }
                Type::Value
            }
            ExprKind::Undefined => Type::Undefined,
            ExprKind::Null => Type::Null,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Num(v, _) => literal_type(*v),
            ExprKind::Str(_) => Type::Str,
            ExprKind::Paren(inner) => self.expr(inner),
            ExprKind::Unary(op, inner) => {
                let t = self.expr(inner);
                match (op, t) {
                    (_, Type::Error) => Type::Error,
                    (UnOp::Not, Type::Bool) => Type::Bool,
                    (UnOp::Neg, Type::IntLit(v)) => Type::IntLit(-v),
                    (UnOp::Neg, Type::NumLit | Type::I32 | Type::F64) => t,
                    (UnOp::BitNot, Type::I32) => Type::I32,
                    (UnOp::BitNot, Type::IntLit(_)) => Type::I32,
                    _ => {
                        let why = match op {
                            UnOp::Not => "`!` needs a boolean (there is no implicit truthiness)",
                            UnOp::Neg => "unary `-` needs i32 or f64",
                            UnOp::BitNot => "`~` needs i32",
                        };
                        self.error(e.span, format!("{why}, found {}", t.name()));
                        Type::Error
                    }
                }
            }
            ExprKind::Binary(op, l, r) => {
                let lt = self.expr(l);
                let rt = self.expr(r);
                self.binary_type(*op, lt, rt, l.span, r.span)
                    .unwrap_or(Type::Value)
            }
            ExprKind::Cond(c, a, b) => {
                self.expect_type(c, Type::Bool, "condition");
                let at = self.expr(a);
                let bt = self.expr(b);
                if let (Type::IntLit(a), Type::IntLit(b)) = (at, bt) {
                    // Two integer literals adapt where used: keep the one that limits the
                    // target most (a negative one for u32, else the larger).
                    Type::IntLit(if a.min(b) < 0.0 { a.min(b) } else { a.max(b) })
                } else if let (Type::IntLit(_) | Type::NumLit, Type::IntLit(_) | Type::NumLit) =
                    (at, bt)
                {
                    Type::NumLit
                } else if assignable(at, bt) {
                    bt
                } else if assignable(bt, at) {
                    at
                } else {
                    self.error(
                        e.span,
                        format!(
                            "the branches have unrelated types {} and {}",
                            at.name(),
                            bt.name()
                        ),
                    );
                    Type::Value
                }
            }
            ExprKind::Call {
                callee,
                callee_span,
                args,
            } => {
                let Some((_, sig)) = self.callees.get(callee) else {
                    let msg = if self.lookup(callee).is_some() {
                        format!(
                            "`{callee}` is a local; call user functions with Call(f, thisArg, ...args)"
                        )
                    } else {
                        format!(
                            "unknown function `{callee}` (not an intrinsic in intrinsics.d.ts or a builtin)"
                        )
                    };
                    self.error(*callee_span, msg);
                    for a in args {
                        self.expr(a);
                    }
                    return Type::Error;
                };
                let sig = sig.clone();
                if sig.this_ty.is_some() {
                    self.error(
                        *callee_span,
                        format!("`{callee}` takes a `this` value and cannot be called directly; move its body into a helper"),
                    );
                }
                let max = if sig.rest.is_some() {
                    usize::MAX
                } else {
                    sig.params.len()
                };
                if args.len() < sig.min_args() || args.len() > max {
                    let want = if sig.rest.is_some() {
                        format!("at least {}", sig.min_args())
                    } else if sig.min_args() == sig.params.len() {
                        sig.params.len().to_string()
                    } else {
                        format!("{} to {}", sig.min_args(), sig.params.len())
                    };
                    self.error(
                        e.span,
                        format!("`{callee}` takes {want} arguments, found {}", args.len()),
                    );
                }
                for (i, a) in args.iter().enumerate() {
                    let want = sig
                        .params
                        .get(i)
                        .map(|p| p.0)
                        .or(sig.rest)
                        .unwrap_or(Type::Value);
                    self.expect_type(a, want, &format!("argument {} of `{callee}`", i + 1));
                }
                sig.ret
            }
        }
    }

    /// The result type of a binary operation, or None after reporting an error.
    fn binary_type(&mut self, op: BinOp, l: Type, r: Type, ls: Span, rs: Span) -> Option<Type> {
        let span = ls.to(rs);
        if l == Type::Error || r == Type::Error {
            return Some(
                if matches!(
                    op,
                    BinOp::Lt
                        | BinOp::Le
                        | BinOp::Gt
                        | BinOp::Ge
                        | BinOp::StrictEq
                        | BinOp::StrictNe
                        | BinOp::And
                        | BinOp::Or
                ) {
                    Type::Bool
                } else {
                    Type::Error
                },
            );
        }
        let fail = |this: &mut Self, why: String| {
            this.error(span, format!("`{}` {why}", op.as_str()));
            None
        };
        match op {
            BinOp::And | BinOp::Or => {
                if l == Type::Bool && r == Type::Bool {
                    Some(Type::Bool)
                } else {
                    fail(
                        self,
                        format!("needs booleans, found {} and {}", l.name(), r.name()),
                    )
                }
            }
            BinOp::StrictEq | BinOp::StrictNe => {
                if l == Type::Void || r == Type::Void {
                    return fail(self, "cannot compare void".into());
                }
                let related = assignable(l, r) || assignable(r, l);
                let mixed_ints = matches!((l, r), (Type::I32, Type::U32) | (Type::U32, Type::I32));
                if related && !mixed_ints {
                    Some(Type::Bool)
                } else {
                    fail(
                        self,
                        format!("compares unrelated types {} and {}", l.name(), r.name()),
                    )
                }
            }
            BinOp::Add if l == Type::Str || r == Type::Str => {
                if l == Type::Str && r == Type::Str {
                    Some(Type::Str)
                } else {
                    fail(
                        self,
                        format!(
                            "concatenates only Str + Str, found {} and {}; convert explicitly",
                            l.name(),
                            r.name()
                        ),
                    )
                }
            }
            _ => {
                let Some(t) = unify_numeric(l, r) else {
                    return fail(
                        self,
                        if l.is_numeric() && r.is_numeric() {
                            format!("mixes {} and {}; convert explicitly", l.name(), r.name())
                        } else {
                            format!("needs numbers, found {} and {}", l.name(), r.name())
                        },
                    );
                };
                match op {
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Some(Type::Bool),
                    BinOp::Add | BinOp::Sub | BinOp::Mul => match (l, r) {
                        // Literal arithmetic folds, so the result can adapt like a literal.
                        (Type::IntLit(a), Type::IntLit(b)) => Some(literal_type(match op {
                            BinOp::Add => a + b,
                            BinOp::Sub => a - b,
                            _ => a * b,
                        })),
                        _ => Some(t),
                    },
                    BinOp::Div | BinOp::Rem => match t {
                        Type::F64 | Type::NumLit => Some(Type::F64),
                        Type::IntLit(_) => Some(Type::F64),
                        _ => fail(
                            self,
                            format!("on {} differs between JS and Rust; use f64", t.name()),
                        ),
                    },
                    BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                        match t {
                            Type::I32 | Type::IntLit(_) => Some(Type::I32),
                            _ => fail(
                                self,
                                format!(
                                    "works on i32 only (JS converts to int32), found {}",
                                    t.name()
                                ),
                            ),
                        }
                    }
                    BinOp::UShr => match t {
                        Type::I32 | Type::IntLit(_) => Some(Type::U32),
                        _ => fail(self, format!("works on i32 only, found {}", t.name())),
                    },
                    _ => unreachable!(),
                }
            }
        }
    }
}

/// The common numeric type of two operands, if any.
pub fn unify_numeric(l: Type, r: Type) -> Option<Type> {
    if !l.is_numeric() || !r.is_numeric() {
        return None;
    }
    match (l, r) {
        (Type::IntLit(_), Type::IntLit(_)) => Some(l),
        (Type::IntLit(_) | Type::NumLit, Type::IntLit(_) | Type::NumLit) => Some(Type::NumLit),
        (Type::IntLit(_) | Type::NumLit, t) | (t, Type::IntLit(_) | Type::NumLit) => {
            let lit = if matches!(l, Type::IntLit(_) | Type::NumLit) {
                l
            } else {
                r
            };
            assignable(lit, t).then_some(t)
        }
        _ if l == r => Some(l),
        _ => None,
    }
}

fn is_true(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Bool(true))
}

/// True if `s` contains a `break` that leaves the loop `s` is the body of.
fn has_break(s: &Stmt) -> bool {
    match s {
        Stmt::Break(_) => true,
        Stmt::If { then, els, .. } => has_break(then) || els.as_deref().is_some_and(has_break),
        Stmt::Block(b) => b.stmts.iter().any(has_break),
        Stmt::Try {
            body,
            catch,
            finally,
            ..
        } => {
            body.stmts.iter().any(has_break)
                || catch
                    .as_ref()
                    .is_some_and(|c| c.body.stmts.iter().any(has_break))
                || finally
                    .as_ref()
                    .is_some_and(|f| f.stmts.iter().any(has_break))
        }
        // A break inside a nested loop belongs to that loop.
        _ => false,
    }
}
