//! TypeScript declarations with runtime meaning (0048): enums, namespaces and
//! `import X = A.B` aliases, compiled like tsc (with `isolatedModules`) and dquickts:
//!
//! ```text
//! var E; (function (E) { E[E["A"] = 0] = "A"; E["S"] = "s"; })(E || (E = {}));
//! var N; (function (N) { N.x = 1; function f() { return N.x } N.f = f; })(N || (N = {}));
//! ```
//!
//! The closure is an arrow-like function whose one parameter (`<ns N>`, unreachable by
//! source names) is the enum or namespace object. A declaration record (`TsDecl`)
//! collects the members of every block of a merged declaration; resolution, which runs
//! after the whole unit is parsed, turns free names in the closures that are members
//! into property accesses on that parameter (`resolve::classify`), so `Merged.f` sees
//! a member that a later block exports. Constant enum initializers are evaluated at
//! compile time (tsc's constant enum expressions); a `const enum` requires them. A
//! namespace that only declares types emits nothing and binds nothing.
use crate::PushOut;

use alloc::vec::Vec;

use super::PResult;
use super::func::{BODY_SCOPE, FuncId, FuncKind, ScopeId, VarKind};
use super::parser::{Parser, syntax};
use super::pattern::Binding;
use crate::heap::Atom;
use crate::lexer::{Kw, TokenKind};

/// An enum member's compile-time value.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum EnumVal {
    Num(f64),
    Str(Vec<u16>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DeclKind {
    Namespace,
    Enum,
    ConstEnum,
}

/// Where a declaration lives: directly in a scope, or as a member of a namespace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Owner {
    Scope(FuncId, ScopeId),
    Member(u32),
}

pub(super) struct TsMember {
    pub name: Atom,
    /// An enum member's constant value (`None`: computed at run time).
    pub value: Option<EnumVal>,
    /// A nested namespace or enum.
    pub child: Option<u32>,
}

/// An enum or namespace, with the members of all its merged blocks.
pub(super) struct TsDecl {
    pub name: Atom,
    pub kind: DeclKind,
    pub owner: Owner,
    /// The closure parameter of its blocks.
    pub param: Atom,
    pub members: Vec<TsMember>,
    /// A type-only namespace: no object, so nothing to merge with or look into.
    pub dead: bool,
}

impl TsDecl {
    fn member(&self, name: Atom) -> Option<&TsMember> {
        self.members.iter().find(|m| m.name == name)
    }
}

/// Precedence of the binary operators of constant enum expressions.
fn const_prec(kind: TokenKind) -> u8 {
    use TokenKind::*;
    match kind {
        Pipe => 1,
        Caret => 2,
        Amp => 3,
        Shl | Shr | UShr => 4,
        Plus | Minus => 5,
        Star | Slash | Percent => 6,
        StarStar => 7,
        _ => 0,
    }
}

fn to_num(v: &EnumVal) -> f64 {
    match v {
        EnumVal::Num(n) => *n,
        EnumVal::Str(s) => {
            crate::num::string_to_number(&crate::num::narrow_units(s.iter().copied())[..])
        }
    }
}

fn to_units(v: &EnumVal) -> Vec<u16> {
    match v {
        EnumVal::Num(n) => crate::num::to_string(*n).as_str().encode_utf16().collect(),
        EnumVal::Str(s) => s.clone(),
    }
}

fn const_binop(op: TokenKind, a: EnumVal, b: EnumVal) -> EnumVal {
    use crate::interp::conv::to_int32;
    if op == TokenKind::Plus && (matches!(a, EnumVal::Str(_)) || matches!(b, EnumVal::Str(_))) {
        let mut s = to_units(&a);
        s.extend(to_units(&b));
        return EnumVal::Str(s);
    }
    let (x, y) = (to_num(&a), to_num(&b));
    let (i, j) = (to_int32(x), to_int32(y));
    EnumVal::Num(match op {
        TokenKind::Plus => x + y,
        TokenKind::Minus => x - y,
        TokenKind::Star => x * y,
        TokenKind::Slash => x / y,
        TokenKind::Percent => x % y,
        TokenKind::StarStar => crate::interp::conv::pow(x, y),
        TokenKind::Pipe => f64::from(i | j),
        TokenKind::Caret => f64::from(i ^ j),
        TokenKind::Amp => f64::from(i & j),
        TokenKind::Shl => f64::from(i.wrapping_shl(j as u32 & 31)),
        TokenKind::Shr => f64::from(i >> (j as u32 & 31)),
        _ => f64::from((i as u32) >> (j as u32 & 31)),
    })
}

impl<'a> Parser<'a, '_> {
    // ---- Declaration records ----------------------------------------------------------

    /// The record of `name` declared in the current scope (or as a member of
    /// `parent`), merged with an earlier declaration if there is one.
    fn ts_get_decl(&mut self, parent: Option<u32>, name: Atom, kind: DeclKind) -> PResult<u32> {
        let found = match parent {
            Some(p) => self.ts_decls[p as usize].member(name).and_then(|m| m.child),
            None => {
                let owner = Owner::Scope(self.f.id, self.scope);
                self.ts_decls
                    .iter()
                    .position(|d| !d.dead && d.name == name && d.owner == owner)
                    .map(|i| i as u32)
            }
        };
        if let Some(d) = found {
            return Ok(d);
        }
        let id = self.ts_decls.len() as u32;
        let param = self.heap.atom_str(&crate::text::cat!("<ns ", id, ">"))?;
        self.ts_decls.push_out(TsDecl {
            name,
            kind,
            owner: match parent {
                Some(p) => Owner::Member(p),
                None => Owner::Scope(self.f.id, self.scope),
            },
            param,
            members: Vec::new(),
            dead: false,
        });
        Ok(id)
    }

    /// A member of namespace `d` declared by `export declare let x`.
    pub(super) fn ts_declare_member(&mut self, d: u32, name: Atom) {
        self.ts_add_member(d, name, None, None);
    }

    /// Adds (or updates) member `name` of declaration `d`.
    fn ts_add_member(&mut self, d: u32, name: Atom, value: Option<EnumVal>, child: Option<u32>) {
        let decl = &mut self.ts_decls[d as usize];
        match decl.members.iter_mut().find(|m| m.name == name) {
            Some(m) => {
                m.value = value;
                if child.is_some() {
                    m.child = child;
                }
            }
            None => decl.members.push_out(TsMember { name, value, child }),
        }
    }

    /// The enum or namespace `name` visible from here, for constant enum expressions:
    /// members of the enclosing namespaces, then declarations of the enclosing scopes.
    pub(super) fn ts_lookup_decl(&self, name: Atom) -> Option<u32> {
        let chain = core::iter::once((&self.f, self.scope))
            .chain(self.stack.iter().rev().map(|s| (&s.f, s.scope)));
        for (f, scope) in chain {
            if let Some(d) = f.ns
                && let Some(c) = self.ts_decls[d as usize].member(name).and_then(|m| m.child)
            {
                return Some(c);
            }
            let found = self.ts_decls.iter().rposition(|d| {
                !d.dead
                    && d.name == name
                    && matches!(d.owner, Owner::Scope(fid, s) if fid == f.id && f.scope_within(scope, s))
            });
            if let Some(i) = found {
                return Some(i as u32);
            }
        }
        None
    }

    /// Pushes the enum or namespace object of the closure being parsed (or an
    /// enclosing one): its parameter.
    fn ts_emit_ns(&mut self, d: u32, pos: u32) {
        let param = self.ts_decls[d as usize].param;
        let r = self.name_ref(param, pos);
        self.code().name_get(r);
    }

    /// Declares the binding of a top-level enum or namespace unless it merges with a
    /// declaration of the same scope (an enum, a namespace, a function, a class): a
    /// `var` at the function level, a `let` in a block (as tsc).
    fn ts_declare_binding(&mut self, d: u32, name: Atom, pos: u32) -> PResult<()> {
        let v = match self.f.find_in_scope(self.scope, name) {
            Some(v) => v,
            None if self.scope == self.f.var_scope => self.declare_var(name, pos)?,
            None => {
                let v = self.declare_lexical(name, VarKind::Let, pos)?;
                self.code().undefined();
                let r = self.init_ref(name, pos);
                self.code().name_init(r);
                self.f.vars[v as usize].init_end = pos;
                v
            }
        };
        self.f.vars[v as usize].ts_decl = Some(d);
        Ok(())
    }

    /// Starts the closure of a block of declaration `d`.
    fn ts_closure_begin(&mut self, d: u32, start: u32) -> PResult<()> {
        self.enter()?;
        self.begin_function(FuncKind::Arrow)?;
        self.f.ns = Some(d);
        self.f.src_start = start;
        let param = self.ts_decls[d as usize].param;
        let v = self.declare_param(param, start)?;
        self.f.vars[v as usize].init_end = 0;
        self.f.length = 1;
        self.code().enter_scope(BODY_SCOPE as u16);
        Ok(())
    }

    /// Ends the closure: its child index, or `None` if `keep` is false (a type-only
    /// namespace: the function is never created).
    fn ts_closure_end(&mut self, keep: bool) -> PResult<Option<u32>> {
        self.code().leave_scope(BODY_SCOPE as u16);
        self.emit_return_undef();
        self.f.src_end = self.prev_end;
        let r = if keep {
            Some(self.end_function()?)
        } else {
            self.discard_function()?;
            None
        };
        self.leave();
        Ok(r)
    }

    /// `(closure)(N || (N = {}))`, or `(closure)(P.N || (P.N = {}))` for a member of
    /// the enclosing namespace `P`.
    fn ts_emit_closure_call(&mut self, child: u32, name: Atom, parent: Option<u32>, pos: u32) {
        self.code().fclosure(child);
        self.code().undefined();
        match parent {
            Some(p) => {
                self.ts_emit_ns(p, pos);
                self.code().get_field(name, 0);
            }
            None => {
                let r = self.name_ref(name, pos);
                self.mark(pos);
                self.code().name_get(r);
            }
        }
        let l = self.new_label();
        self.code().dup();
        self.code().jump_if_true(l);
        self.code().drop();
        self.code().object();
        self.code().dup();
        match parent {
            Some(p) => {
                self.ts_emit_ns(p, pos);
                self.code().swap();
                self.code().put_field(name, 0);
            }
            None => {
                let r = self.name_ref(name, pos);
                self.mark(pos);
                self.code().name_put(r);
            }
        }
        self.place(l);
        self.mark(pos);
        self.code().call(1);
        self.code().drop();
    }

    /// The name of an enum or namespace (an identifier).
    fn ts_decl_name(&mut self) -> PResult<Atom> {
        if !self.is(TokenKind::Ident) {
            return syntax(self.tok.start, "identifier expected");
        }
        let a = self.ident_atom()?;
        self.next()?;
        Ok(a)
    }

    // ---- Enums ---------------------------------------------------------------------------

    /// `enum E { ... }` / `const enum E { ... }` at `enum`. `parent`: the enclosing
    /// namespace of an `export enum`.
    pub(super) fn ts_enum(&mut self, is_const: bool, parent: Option<u32>) -> PResult<()> {
        let start = self.tok.start;
        self.next()?;
        let name_pos = self.tok.start;
        let name = self.ts_decl_name()?;
        let kind = if is_const {
            DeclKind::ConstEnum
        } else {
            DeclKind::Enum
        };
        let d = self.ts_get_decl(parent, name, kind)?;
        self.ts_decls[d as usize].kind = kind;
        match parent {
            Some(p) => self.ts_add_member(p, name, None, Some(d)),
            None => self.ts_declare_binding(d, name, name_pos)?,
        }
        self.ts_closure_begin(d, start)?;
        self.expect(TokenKind::LBrace, "{")?;
        let mut auto = Some(0.0);
        while !self.is(TokenKind::RBrace) {
            let mtok = self.tok;
            let member = match mtok.kind {
                TokenKind::String => {
                    let units = self.cooked_string();
                    self.atom_of_units(&units)?
                }
                k if k.is_identifier_name() => self.ident_atom()?,
                _ => return syntax(mtok.start, "invalid enum member name"),
            };
            self.next()?;
            let value = if self.is(TokenKind::Assign) {
                self.next()?;
                let mut v = None;
                self.ts_try(|p| {
                    let c = p.ts_const_expr(d, 1)?;
                    let ok = matches!(p.tok.kind, TokenKind::Comma | TokenKind::RBrace);
                    if ok {
                        v = Some(c);
                    }
                    Ok(ok)
                });
                if v.is_none() && is_const {
                    return syntax(
                        self.tok.start,
                        "const enum member initializers must be constant expressions",
                    );
                }
                v
            } else {
                match auto {
                    Some(n) => Some(EnumVal::Num(n)),
                    None => return syntax(self.tok.start, "enum member must have initializer"),
                }
            };
            self.ts_emit_ns(d, mtok.start);
            match &value {
                Some(EnumVal::Str(s)) => {
                    // `E["A"] = "x"`: no reverse mapping for string members.
                    let idx = self.string_const(s)?;
                    self.code().push_const(idx);
                    self.mark(mtok.start);
                    self.code().put_field(member, 0);
                }
                _ => {
                    // `E[E["A"] = v] = "A"`
                    self.ts_emit_ns(d, mtok.start);
                    match &value {
                        Some(EnumVal::Num(n)) => self.emit_number(*n),
                        _ => {
                            self.parse_assign(false)?;
                        }
                    }
                    self.code().insert2();
                    self.mark(mtok.start);
                    self.code().put_field(member, 0);
                    let text: Vec<u16> = self.heap.atom_name(member).encode_utf16().collect();
                    let idx = self.string_const(&text)?;
                    self.code().push_const(idx);
                    self.code().put_elem();
                }
            }
            auto = match &value {
                Some(EnumVal::Num(n)) => Some(n + 1.0),
                _ => None,
            };
            self.ts_add_member(d, member, value, None);
            if !self.eat(TokenKind::Comma)? {
                break;
            }
        }
        if !self.is(TokenKind::RBrace) {
            return syntax(self.tok.start, "expecting '}'");
        }
        self.next()?;
        let child = self.ts_closure_end(true)?.expect("kept closure");
        self.ts_emit_closure_call(child, name, parent, start);
        self.ts_type_only = false;
        Ok(())
    }

    /// A constant enum expression (speculative: an error means "not constant").
    fn ts_const_expr(&mut self, d: u32, min: u8) -> PResult<EnumVal> {
        self.enter()?;
        let r = self.ts_const_expr_inner(d, min);
        self.leave();
        r
    }

    fn ts_const_expr_inner(&mut self, d: u32, min: u8) -> PResult<EnumVal> {
        let mut a = self.ts_const_unary(d)?;
        loop {
            self.operator_tok();
            let op = self.tok.kind;
            let prec = const_prec(op);
            if prec == 0 || prec < min {
                return Ok(a);
            }
            self.next()?;
            // `**` is right-associative.
            let b = self.ts_const_expr(
                d,
                if op == TokenKind::StarStar {
                    prec
                } else {
                    prec + 1
                },
            )?;
            a = const_binop(op, a, b);
        }
    }

    fn ts_const_unary(&mut self, d: u32) -> PResult<EnumVal> {
        let op = self.tok.kind;
        if matches!(op, TokenKind::Plus | TokenKind::Minus | TokenKind::Tilde) {
            self.next()?;
            let n = to_num(&self.ts_const_unary(d)?);
            return Ok(EnumVal::Num(match op {
                TokenKind::Minus => -n,
                TokenKind::Tilde => f64::from(!crate::interp::conv::to_int32(n)),
                _ => n,
            }));
        }
        self.ts_const_primary(d)
    }

    /// A literal, `(expr)`, a member of the enum being defined, or `E.A` / `E["A"]` of
    /// another enum (through namespaces: `N.E.A`).
    fn ts_const_primary(&mut self, d: u32) -> PResult<EnumVal> {
        let tok = self.tok;
        let v = match tok.kind {
            TokenKind::Number => {
                let lit = self.lx.number_literal(&tok);
                EnumVal::Num(crate::num::literal_digits_value(
                    lit.base.radix(),
                    lit.digits,
                ))
            }
            TokenKind::String => EnumVal::Str(self.cooked_string()),
            TokenKind::NoSubstTemplate => {
                let mut units = Vec::new();
                self.lx.cook_template(&tok, &mut units)?;
                EnumVal::Str(units)
            }
            TokenKind::LParen => {
                self.next()?;
                let v = self.ts_const_expr(d, 1)?;
                if !self.is(TokenKind::RParen) {
                    return self.ts_error();
                }
                v
            }
            TokenKind::Ident => {
                let name = self.ident_atom()?;
                if let Some(m) = self.ts_decls[d as usize].member(name) {
                    let Some(v) = m.value.clone() else {
                        return self.ts_error();
                    };
                    self.next()?;
                    return Ok(v);
                }
                let Some(mut ns) = self.ts_lookup_decl(name) else {
                    return self.ts_error();
                };
                loop {
                    self.next()?;
                    let key = match self.tok.kind {
                        TokenKind::Dot => {
                            self.next()?;
                            if !self.tok.kind.is_identifier_name() {
                                return self.ts_error();
                            }
                            self.ident_atom()?
                        }
                        TokenKind::LBracket => {
                            self.next()?;
                            if !self.is(TokenKind::String) {
                                return self.ts_error();
                            }
                            let units = self.cooked_string();
                            let a = self.atom_of_units(&units)?;
                            self.next()?;
                            if !self.is(TokenKind::RBracket) {
                                return self.ts_error();
                            }
                            a
                        }
                        _ => return self.ts_error(),
                    };
                    let decl = &self.ts_decls[ns as usize];
                    let Some(m) = decl.member(key) else {
                        return self.ts_error();
                    };
                    if decl.kind != DeclKind::Namespace {
                        let Some(v) = m.value.clone() else {
                            return self.ts_error();
                        };
                        break v;
                    }
                    match m.child {
                        Some(c) => ns = c,
                        None => return self.ts_error(),
                    }
                }
            }
            _ => return self.ts_error(),
        };
        self.next()?;
        Ok(v)
    }

    // ---- Namespaces ----------------------------------------------------------------------

    /// `namespace A.B { ... }` / `module A { ... }` at the name. `parent`: the
    /// enclosing namespace (`export namespace`, or the `A` of `A.B`). Sets
    /// `ts_type_only` if it only declares types (then nothing is emitted).
    pub(super) fn ts_namespace(&mut self, parent: Option<u32>) -> PResult<()> {
        let start = self.tok.start;
        let name = self.ts_decl_name()?;
        let fresh = self.ts_decls.len() as u32;
        let d = self.ts_get_decl(parent, name, DeclKind::Namespace)?;
        self.ts_closure_begin(d, start)?;
        let has_value = if self.is(TokenKind::Dot) {
            // `namespace A.B {}` is `namespace A { export namespace B {} }`.
            self.next()?;
            self.ts_namespace(Some(d))?;
            !self.ts_type_only
        } else {
            self.expect(TokenKind::LBrace, "{")?;
            let mut has_value = false;
            while !self.is(TokenKind::RBrace) {
                if self.is(TokenKind::Eof) {
                    return syntax(self.tok.start, "expecting '}'");
                }
                self.ts_type_only = false;
                self.ts_namespace_element(d)?;
                if !self.ts_type_only {
                    has_value = true;
                }
            }
            self.next()?;
            has_value
        };
        let child = self.ts_closure_end(has_value)?;
        let Some(child) = child else {
            if d == fresh {
                self.ts_decls[d as usize].dead = true;
            }
            self.ts_type_only = true;
            return Ok(());
        };
        match parent {
            Some(p) => self.ts_add_member(p, name, None, Some(d)),
            None => self.ts_declare_binding(d, name, start)?,
        }
        self.ts_emit_closure_call(child, name, parent, start);
        self.ts_type_only = false;
        Ok(())
    }

    /// `N.x = x` for an exported function or class of namespace `d`.
    fn ts_emit_ns_export(&mut self, d: u32, name: Atom, pos: u32) {
        self.ts_add_member(d, name, None, None);
        self.ts_emit_ns(d, pos);
        let r = self.name_ref(name, pos);
        self.code().name_get(r);
        self.mark(pos);
        self.code().put_field(name, 0);
    }

    /// One statement of the body of namespace `d`.
    fn ts_namespace_element(&mut self, d: u32) -> PResult<()> {
        if !self.is(TokenKind::Export) {
            return self.parse_statement_list_item();
        }
        self.next()?;
        let tok = self.tok;
        match tok.kind {
            TokenKind::Const if self.peek().kind == TokenKind::Enum => {
                self.next()?;
                return self.ts_enum(true, Some(d));
            }
            TokenKind::Const | TokenKind::Var => {
                self.next()?;
                return self.ts_export_vars(d, tok.kind == TokenKind::Const);
            }
            TokenKind::Ident if tok.is_contextual(Kw::Let) => {
                self.next()?;
                return self.ts_export_vars(d, false);
            }
            TokenKind::Function => return self.ts_export_function(d, tok.start, false),
            TokenKind::Ident if self.at_async_function() => {
                return self.ts_export_function(d, tok.start, true);
            }
            TokenKind::Class => return self.ts_export_class(d),
            TokenKind::Enum => return self.ts_enum(false, Some(d)),
            TokenKind::Import => {
                self.next()?;
                return self.ts_import_alias(Some(d));
            }
            _ => {}
        }
        if tok.kind == TokenKind::Ident && !tok.has_escape() {
            match tok.kw {
                Kw::Abstract if self.peek().kind == TokenKind::Class => {
                    self.next()?;
                    self.ts_erase(tok.start);
                    return self.ts_export_class(d);
                }
                Kw::Namespace | Kw::Module => {
                    self.next()?;
                    return self.ts_namespace(Some(d));
                }
                Kw::Declare => return self.ts_declare(Some(d)),
                Kw::Type => {
                    self.ts_type_alias()?;
                    self.ts_type_only = true;
                    return Ok(());
                }
                Kw::Interface => {
                    self.ts_interface()?;
                    self.ts_type_only = true;
                    return Ok(());
                }
                _ => {}
            }
        }
        syntax(tok.start, "unsupported export in a namespace")
    }

    /// `export function f() {}` in namespace `d` (an overload signature is type-only).
    fn ts_export_function(&mut self, d: u32, start: u32, is_async: bool) -> PResult<()> {
        if self.ts_statement()? {
            // An overload signature.
            return Ok(());
        }
        if is_async {
            self.next()?;
        }
        // The name, for the export.
        let mut lx = self.lx;
        let mut name_tok = lx.next_token()?;
        if name_tok.kind == TokenKind::Star {
            name_tok = lx.next_token()?;
        }
        self.parse_function_declaration(start, is_async)?;
        let bytes = self.lx.ident_value(&name_tok, &mut self.buf8);
        let s = core::str::from_utf8(bytes).unwrap_or("\u{FFFD}");
        let name = self.heap.atom_str(s)?;
        self.ts_emit_ns_export(d, name, name_tok.start);
        Ok(())
    }

    /// `export class K {}` in namespace `d`.
    fn ts_export_class(&mut self, d: u32) -> PResult<()> {
        let next = self.peek();
        if next.kind != TokenKind::Ident {
            return syntax(next.start, "class name expected");
        }
        let mut lx = self.lx;
        let name_tok = lx.next_token()?;
        self.parse_class(true, Atom::NULL)?;
        let bytes = self.lx.ident_value(&name_tok, &mut self.buf8);
        let s = core::str::from_utf8(bytes).unwrap_or("\u{FFFD}");
        let name = self.heap.atom_str(s)?;
        self.ts_emit_ns_export(d, name, name_tok.start);
        Ok(())
    }

    /// `export let/const/var ...` in namespace `d`: the names are members (properties of
    /// the namespace object, for the namespace's own code too), not local bindings.
    fn ts_export_vars(&mut self, d: u32, is_const: bool) -> PResult<()> {
        loop {
            if matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
                let start = self.tok.start;
                let mark = self.pattern_vars.len();
                self.deferred_pattern(Binding::Member, |p| {
                    p.ts_annotation()?;
                    if !p.is(TokenKind::Assign) {
                        return syntax(start, "variable name expected");
                    }
                    p.next()?;
                    p.parse_assign(false)?;
                    Ok(())
                })?;
                let end = self.prev_end;
                self.end_pattern_bindings(mark, end);
            } else {
                let pos = self.tok.start;
                let name = self.binding_name()?;
                self.next()?;
                self.ts_eat_marker(TokenKind::Bang)?;
                self.ts_annotation()?;
                self.ts_add_member(d, name, None, None);
                if self.eat(TokenKind::Assign)? {
                    self.parse_named(name, |p| p.parse_assign(false))?;
                    let r = self.name_ref(name, pos);
                    self.mark(pos);
                    self.code().name_put(r);
                } else if is_const {
                    return syntax(self.tok.start, "missing initializer for const variable");
                }
            }
            if !self.eat(TokenKind::Comma)? {
                break;
            }
        }
        self.expect_semicolon()
    }

    /// Records a member declared by a pattern of `export const {a} = ...` (the name of
    /// a `Binding::Member` target).
    pub(super) fn ts_pattern_member(&mut self, name: Atom) {
        if let Some(d) = self.f.ns {
            self.ts_add_member(d, name, None, None);
        }
    }

    /// `X = A.B.C;` after `import` (`ns`: the namespace of an `export import`): a const
    /// binding to the entity, or a member. The root is read like `typeof` and the
    /// members through optional chaining, so an alias of a type-only namespace does not
    /// throw (tsc drops such aliases).
    pub(super) fn ts_import_alias(&mut self, ns: Option<u32>) -> PResult<()> {
        let pos = self.tok.start;
        let name = self.ts_decl_name()?;
        self.expect(TokenKind::Assign, "=")?;
        if self.lx.text(&self.tok) == b"require"
            && self.is(TokenKind::Ident)
            && self.peek().kind == TokenKind::LParen
        {
            return syntax(
                self.tok.start,
                "'import ... = require()' is not supported (no CommonJS modules), use 'import ... from'",
            );
        }
        if !self.is(TokenKind::Ident) {
            return syntax(self.tok.start, "identifier expected");
        }
        let root = self.ident_atom()?;
        let r = self.name_ref(root, self.tok.start);
        self.mark(self.tok.start);
        self.code().name_typeof(r);
        self.next()?;
        let l = self.new_label();
        while self.is(TokenKind::Dot) {
            self.next()?;
            if !self.tok.kind.is_identifier_name() {
                return syntax(self.tok.start, "identifier expected");
            }
            let a = self.ident_atom()?;
            self.code().dup();
            self.code().is_nullish();
            self.code().jump_if_true(l);
            self.mark(self.tok.start);
            self.code().get_field(a, 0);
            self.next()?;
        }
        self.place(l);
        match ns {
            Some(d) => {
                self.ts_add_member(d, name, None, None);
                self.ts_emit_ns(d, pos);
                self.code().swap();
                self.code().put_field(name, 0);
                self.ts_type_only = false;
            }
            None => {
                let v = self.declare_lexical(name, VarKind::Const, pos)?;
                let r = self.init_ref(name, pos);
                self.code().name_init(r);
                self.f.vars[v as usize].init_end = self.prev_end;
                // An unexported alias does not make a namespace non-empty.
                self.ts_type_only = true;
            }
        }
        self.expect_semicolon()
    }

    /// Is the current `import` the start of `import X = ...`?
    pub(super) fn ts_at_import_alias(&mut self) -> bool {
        self.ts_lookahead(|p| {
            p.next()?;
            if !p.is(TokenKind::Ident) || p.tok.is_contextual(Kw::Type) {
                return Ok(false);
            }
            p.next()?;
            Ok(p.is(TokenKind::Assign))
        })
    }
}
