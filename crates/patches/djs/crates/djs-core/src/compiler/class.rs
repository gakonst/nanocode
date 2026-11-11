//! Classes (0040, docs/compiler.md 7.9).
//!
//! A class compiles in the enclosing function, in two nested scopes: the class scope
//! with the inner class binding (in TDZ until the class is defined), and inside it the
//! body scope with the private names and the hidden bindings. The heritage is
//! evaluated in the class scope only, so it sees the outer class's private names.
//!
//! ```text
//! [heritage; dup; call_intrinsic %ClassHeritage 1 | undefined; undefined]
//! fclosure ctor               ; the constructor's child index is reserved first
//! define_class flags          ; -> ctor proto
//! <elements>                  ; methods on proto (static ones: swap; ...; swap),
//!                             ; computed field keys into hidden bindings
//! [fclosure fields; set_home_object | undefined]; name_init <fields>
//! drop; [dup; name_init C]    ; -> ctor
//! [fclosure static; set_home_object; pick 1; call 0; drop]
//! ```
//!
//! Field initializers are two functions per class (instance and static, kind
//! `FieldInit`), parked while the class body is parsed and resumed for each
//! initializer. Each starts with a jump to the end, where the private methods and
//! accessors are installed once all are known, and back: PrivateMethodOrAccessorAdd
//! comes before the fields. The instance one is the hidden binding `<fields N>` that
//! constructors call: a base constructor at entry, a derived one after `super()`.
//! Static blocks are functions of their own, called by the static initializer.
use crate::PushOut;
use crate::bytecode::intrinsics::intrinsic_id_of;

use alloc::vec::Vec;

use super::PResult;
use super::decorators::{
    DEC_ACCESSOR, DEC_FIELD, DEC_GETTER, DEC_METHOD, DEC_SETTER, DecElem, DecKey, DecParams,
    LegacyElem, Synth, Thunk,
};
use super::func::{FuncKind, FuncState, ScopeId, VarId, VarKind};
use super::parser::{ClassSpan, ClassSpanKind, Parser, syntax};
use super::ts_meta::TsMeta;
use crate::bytecode::Label;
use crate::heap::Atom;
use crate::lexer::{Kw, TokenKind};

/// A class being parsed: its private names, for the early errors.
#[derive(Default)]
pub(super) struct ClassCtx {
    privates: Vec<Private>,
    /// Private name references (`#x` and position) to check at the class end: each
    /// must be declared by this class or an enclosing one.
    refs: Vec<(Atom, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PrivKind {
    Field,
    Method,
    Getter,
    Setter,
    /// A getter and a setter.
    Accessor,
}

struct Private {
    name: Atom,
    kind: PrivKind,
    is_static: bool,
}

/// A private method or accessor to install: the private name and the hidden bindings
/// of its closures (method or getter, setter).
struct Install {
    name: Atom,
    kind: PrivKind,
    get: Option<VarId>,
    set: Option<VarId>,
}

/// A parked field initializer function.
struct Init {
    /// Boxed, like a decorator thunk's (D1941).
    f: alloc::boxed::Box<FuncState>,
    /// Where the initializers continue after the private method installation.
    body: Label,
    /// The jump target at the end: the private method installation.
    methods: Label,
}

/// What `parse_class` collects while it parses the body.
struct ClassBuild {
    start: u32,
    name: Atom,
    derived: bool,
    scope: ScopeId,
    ctor_slot: u32,
    has_ctor: bool,
    fields_atom: Atom,
    inst: Option<Init>,
    stat: Option<Init>,
    inst_methods: Vec<Install>,
    stat_methods: Vec<Install>,
    keys: u32,
    /// Decorated elements in source order (compiler/decorators.rs).
    decs: Vec<DecElem>,
    /// The hidden binding of the decorators of the element being parsed.
    pending: Option<Atom>,
    /// Counter for the decorators' hidden bindings.
    hidden: u32,
    /// The hidden binding of the class decorators.
    class_decs: Option<Atom>,
    /// The decoration state, once the elements are decorated, and whether the instance
    /// field initializer runs instance extra initializers.
    state: Option<Atom>,
    inst_extras: bool,
    /// TypeScript legacy decorators (`experimentalDecorators`), with metadata.
    legacy: bool,
    metadata: bool,
    /// Legacy: the class decorators' thunk, the one of the element being parsed, the
    /// decorated elements in source order, and the constructor's parameter types.
    class_thunk: Option<Thunk>,
    pending_thunk: Option<Thunk>,
    legacy_elems: Vec<LegacyElem>,
    ctor_types: Vec<TsMeta>,
}

/// A method being parsed: its key, start, `static` and decorators.
struct Elem {
    key: Key,
    start: u32,
    is_static: bool,
    decs: Option<Atom>,
    thunk: Option<Thunk>,
}

/// The name of an element.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Atom(Atom),
    Private(Atom),
    /// On the stack.
    Computed,
}

impl Parser<'_, '_> {
    /// A class declaration (`decl`, binding its name in the current scope) or
    /// expression (the class on the stack), with the current token at `class`.
    /// `hint` names an anonymous class expression.
    pub(super) fn parse_class(&mut self, decl: bool, hint: Atom) -> PResult<()> {
        self.enter_n(super::parser::CLASS_WEIGHT)?;
        // What a class definition keeps on the stack (heritage, constructor, prototype,
        // computed keys) is not described in suspension maps yet (D2563).
        self.unsupported_begin("class");
        self.class_depth += 1;
        let r = self.parse_class_inner(decl, hint);
        self.class_depth -= 1;
        self.unsupported_end();
        self.leave_n(super::parser::CLASS_WEIGHT);
        r
    }

    fn parse_class_inner(&mut self, decl: bool, hint: Atom) -> PResult<()> {
        self.enter()?;
        let decorated = self.class_decs.take();
        let mut class_thunk = self.class_thunk.take();
        let decorated_at = decorated.or(class_thunk.as_ref().map(|t| t.start));
        let start = self.tok.start;
        self.next()?;
        let mut name = Atom::NULL;
        let mut outer = None;
        // `class implements I {}` (TypeScript) is anonymous.
        if self.is(TokenKind::Ident) && !(self.ts && self.tok.is_contextual(Kw::Implements)) {
            let tok = self.tok;
            if tok.is_escaped_reserved()
                || tok.kw.is_strict_reserved()
                || tok.kw == Kw::Yield
                || (tok.kw == Kw::Await && self.await_reserved())
            {
                return syntax(tok.start, "class statement requires a name");
            }
            name = self.binding_name()?;
            self.next()?;
            if decl {
                let v = self.declare_lexical(name, VarKind::Let, tok.start)?;
                self.syn_decl(v, super::smap_syntax::DeclKind::Class);
                outer = Some(v);
            }
        } else if decl {
            return syntax(self.tok.start, "class statement requires a name");
        }
        self.ts_type_params(super::ts::TP_IN_OUT | super::ts::TP_CONST)?;
        let outer_scope = self.scope;
        let class_scope = self.push_scope(super::func::ScopeKind::Class)?;
        if let Some(t) = &mut class_thunk {
            // Legacy class decorators are evaluated in the class scope (as tsc).
            t.f.parent = Some((self.f.id, class_scope));
        }
        let class_decs = match decorated {
            Some(_) => {
                // The class decorators' array (evaluated before `class`).
                let a = self.heap.atom_str("<class decs>")?;
                self.f.add_var(a, VarKind::Hidden, class_scope);
                let r = self.init_ref(a, start);
                self.code().name_init(r);
                Some(a)
            }
            None => None,
        };
        let inner = if name.is_null() {
            None
        } else {
            Some(self.f.add_var(name, VarKind::Const, class_scope))
        };
        let derived = self.eat(TokenKind::Extends)?;
        if derived {
            let pos = self.tok.start;
            self.parse_lhs(false)?;
            // `extends Base<T>`
            let args = self.tok.start;
            if self.ts && self.ts_skip_type_args(false)? {
                self.ts_erase(args);
            }
            self.code().dup();
            let id = intrinsic_id_of!("ClassHeritage");
            self.mark(pos);
            self.code().call_intrinsic(id, 1);
        } else {
            self.code().undefined();
            self.code().undefined();
        }
        if self.ts && self.ts_at(Kw::Implements) {
            let start = self.tok.start;
            loop {
                self.next()?;
                self.ts_skip_type()?;
                if !self.is(TokenKind::Comma) {
                    break;
                }
            }
            self.ts_erase(start);
        }
        let scope = self.push_scope(super::func::ScopeKind::Class)?;
        let fields_atom =
            self.heap
                .atom_str(&crate::text::cat!("<fields ", self.classes.len(), ">"))?;
        self.f.add_var(fields_atom, VarKind::Hidden, scope);
        self.classes.push_out(ClassCtx {
            privates: Vec::new(),
            refs: Vec::new(),
        });
        let ctor_slot = self.reserve_child();
        self.code().fclosure(ctor_slot);
        self.code().define_class(u8::from(derived));
        let mut b = ClassBuild {
            start,
            name: if name.is_null() { hint } else { name },
            derived,
            scope,
            ctor_slot,
            has_ctor: false,
            fields_atom,
            inst: None,
            stat: None,
            inst_methods: Vec::new(),
            stat_methods: Vec::new(),
            keys: 0,
            decs: Vec::new(),
            pending: None,
            hidden: 0,
            class_decs,
            state: None,
            inst_extras: false,
            legacy: self.legacy_decorators(),
            metadata: self.decorator_metadata(),
            class_thunk,
            pending_thunk: None,
            legacy_elems: Vec::new(),
            ctor_types: Vec::new(),
        };
        self.expect(TokenKind::LBrace, "{")?;
        if derived {
            // strip-types: the heritage was the last JS visited before the members
            // (ts-blank-space does not visit the class name).
            self.ts_semi = true;
        }
        while !self.is(TokenKind::RBrace) {
            if self.eat(TokenKind::Semicolon)? {
                continue;
            }
            if self.is(TokenKind::Eof) {
                // qjs: a member expected at the end of input (D2071).
                return syntax(self.tok.start, "invalid property name");
            }
            let mut dec_start = None;
            if self.is(TokenKind::At) {
                let pos = self.tok.start;
                dec_start = Some(pos);
                if b.legacy {
                    b.pending_thunk = Some(self.parse_legacy_decorators(None)?);
                } else {
                    let a = self.dec_hidden(b.scope, &mut b.hidden)?;
                    self.parse_decorators()?;
                    let r = self.init_ref(a, pos);
                    self.code().name_init(r);
                    b.pending = Some(a);
                }
            }
            // Index signatures, overloads, `declare` and `abstract` members: no code
            // (their decorators are ignored).
            let member = self.tok.start;
            if self.ts && self.ts_skip_class_member()? {
                b.pending = None;
                if let Some(t) = b.pending_thunk.take() {
                    self.discard_thunk(t)?;
                }
                // ts-blank-space erases them with the member.
                if let (Some(d), Some(e)) = (dec_start, &mut self.erased)
                    && let Some(r) = e.iter_mut().rev().find(|r| r.0 == member)
                {
                    r.0 = d;
                }
                continue;
            }
            self.parse_class_element(&mut b)?;
            self.ts_note_stmt_end();
        }
        self.next()?;
        if !b.has_ctor {
            self.default_ctor(&b)?;
        }
        let end = self.prev_end;
        // The constructor's source text is the whole class.
        let ctor_id = self.f.children[b.ctor_slot as usize];
        if let Some(Some(c)) = self.done.get_mut(ctor_id as usize) {
            c.src_start = start;
            c.src_end = end;
        }

        if b.class_decs.is_some() || !b.decs.is_empty() {
            let state = self.dec_hidden(b.scope, &mut b.hidden)?;
            let elems = core::mem::take(&mut b.decs);
            self.emit_decorate_elements(&elems, state, derived, start)?;
            b.state = Some(state);
            b.inst_extras = elems
                .iter()
                .any(|d| !d.is_static && matches!(d.kind, DEC_METHOD | DEC_GETTER | DEC_SETTER));
            if b.inst_extras && b.inst.is_none() {
                b.inst = Some(self.new_init(b.start)?);
            }
        }

        // `ctor proto` -> the instance initializer, the inner binding, the statics.
        let methods = core::mem::take(&mut b.inst_methods);
        let extras = if b.inst_extras { b.state } else { None };
        match b.inst.take() {
            Some(init) => {
                let child = self.finish_init(init, &methods, extras)?;
                self.code().fclosure(child);
                self.code().set_home_object();
            }
            None => self.code().undefined(),
        }
        let r = self.init_ref(fields_atom, end);
        self.code().name_init(r);
        self.code().drop();
        if let Some(state) = b.state {
            // `ctor` -> `ctor new_class`.
            self.emit_decorate_class(b.class_decs, b.name, state, start)?;
        }
        if let Some(v) = inner {
            self.code().dup();
            let r = self.init_ref(name, end);
            self.code().name_init(r);
            self.f.vars[v as usize].init_end = end;
        }
        if let Some(state) = b.state {
            self.emit_state_extras(state, super::decorators::SLOT_STATIC_EXTRAS, start);
        }
        let methods = core::mem::take(&mut b.stat_methods);
        if let Some(init) = b.stat.take() {
            let child = self.finish_init(init, &methods, None)?;
            if b.state.is_some() {
                // `this` is the new class, the home object the class.
                self.code().swap();
                self.code().fclosure(child);
                self.code().set_home_object();
                self.code().pick(2);
                self.mark(start);
                self.code().call(0);
                self.code().drop();
                self.code().drop();
            } else {
                self.code().fclosure(child);
                self.code().set_home_object();
                self.code().pick(1);
                self.mark(start);
                self.code().call(0);
                self.code().drop();
            }
        } else if b.state.is_some() {
            self.code().nip();
        }
        if let Some(state) = b.state {
            self.emit_state_extras(state, super::decorators::SLOT_CLASS_EXTRAS, start);
        }
        if b.class_thunk.is_some() || !b.legacy_elems.is_empty() {
            // Legacy decorators: the instance elements, the static ones, the class.
            if outer.is_some() {
                // As tsc, the declaration's binding holds the class meanwhile.
                self.code().dup();
                let scope = core::mem::replace(&mut self.scope, outer_scope);
                let r = self.init_ref(name, start);
                self.scope = scope;
                self.code().name_init(r);
            }
            let elems = core::mem::take(&mut b.legacy_elems);
            let (stat, inst): (Vec<_>, Vec<_>) = elems.into_iter().partition(|e| e.is_static);
            for e in inst.into_iter().chain(stat) {
                self.emit_legacy_apply(e)?;
            }
            if let Some(mut t) = b.class_thunk.take() {
                if b.metadata && b.has_ctor {
                    let types = core::mem::take(&mut b.ctor_types);
                    t = self.thunk_ctor_metadata(t, &types)?;
                }
                self.emit_legacy_apply_class(t)?;
                if inner.is_some() {
                    // The class name refers to the decorated class.
                    self.code().dup();
                    let r = self.init_ref(name, end);
                    self.code().name_init(r);
                }
            }
        }
        self.pop_scope();
        self.pop_scope();
        self.check_private_refs()?;
        if let Some(v) = outer {
            let r = self.init_ref(name, start);
            self.mark(start);
            self.code().name_init(r);
            self.f.vars[v as usize].init_end = end;
        } else if name.is_null() {
            self.last_anon_fn = Some((decorated_at.unwrap_or(start), end));
            if !hint.is_null() {
                self.named_fn = Some(self.f.children[b.ctor_slot as usize]);
            }
        }
        self.leave();
        Ok(())
    }

    /// Checks the private name references of the class just finished: those it does
    /// not declare go to the enclosing class, or are errors.
    fn check_private_refs(&mut self) -> PResult<()> {
        let ctx = self.classes.pop().expect("class context");
        for (atom, pos) in ctx.refs {
            if ctx.privates.iter().any(|p| p.name == atom) {
                continue;
            }
            match self.classes.last_mut() {
                Some(outer) => outer.refs.push_out((atom, pos)),
                None => {
                    let name = self.heap.atom_name(atom);
                    return syntax(
                        pos,
                        crate::text::cat!("undefined private field '", name, "'"),
                    );
                }
            }
        }
        Ok(())
    }

    /// A reference to the private name at the current token (`#x`): the binding of
    /// its private symbol, checked at the end of the class.
    pub(super) fn private_ref(&mut self, pos: u32) -> PResult<u32> {
        let atom = self.ident_atom()?;
        match self.classes.last_mut() {
            Some(c) => c.refs.push_out((atom, pos)),
            None => {
                let name = self.heap.atom_name(atom);
                return syntax(
                    pos,
                    crate::text::cat!("undefined private field '", name, "'"),
                );
            }
        }
        Ok(self.name_ref(atom, pos))
    }

    /// One class element (not `;`).
    fn parse_class_element(&mut self, b: &mut ClassBuild) -> PResult<()> {
        let decs = b.pending.take();
        let thunk = b.pending_thunk.take();
        let has_decs = decs.is_some() || thunk.is_some();
        let first_modifier = if self.ts {
            self.ts_member_modifiers()?
        } else {
            None
        };
        let mut is_static = false;
        if self.tok.kw == Kw::Static && self.tok.kind == TokenKind::Ident && !self.tok.has_escape()
        {
            let next = self.peek();
            if next.kind == TokenKind::LBrace {
                self.next()?;
                if has_decs {
                    return syntax(self.tok.start, "decorators are not valid here");
                }
                return self.parse_static_block(b);
            }
            let name = matches!(
                next.kind,
                TokenKind::LParen | TokenKind::Assign | TokenKind::Semicolon | TokenKind::RBrace
            ) || (self.ts
                && matches!(
                    next.kind,
                    TokenKind::Colon | TokenKind::Question | TokenKind::Bang | TokenKind::Lt
                ));
            if !name {
                self.next()?;
                is_static = true;
                if self.ts {
                    self.ts_member_modifiers()?;
                }
            }
        }
        let start = self.tok.start;
        let name_start = |k: crate::lexer::Token| {
            k.kind.is_identifier_name()
                || matches!(
                    k.kind,
                    TokenKind::String
                        | TokenKind::Number
                        | TokenKind::BigInt
                        | TokenKind::LBracket
                        | TokenKind::PrivateName
                )
        };
        let mut is_async = false;
        let mut is_gen = false;
        let mut accessor = None;
        // An auto-accessor: `accessor` [no LineTerminator here] ClassElementName.
        let mut auto = false;
        let tok = self.tok;
        if tok.kind == TokenKind::Ident && !tok.has_escape() {
            let next = self.peek();
            if tok.kw == Kw::Accessor && !next.newline_before() && name_start(next) {
                self.next()?;
                auto = true;
            } else if tok.kw == Kw::Async
                && !next.newline_before()
                && (name_start(next) || next.kind == TokenKind::Star)
            {
                self.next()?;
                is_async = true;
            } else if matches!(tok.kw, Kw::Get | Kw::Set) && name_start(next) {
                self.next()?;
                accessor = Some(if tok.kw == Kw::Get {
                    FuncKind::Getter
                } else {
                    FuncKind::Setter
                });
            }
        }
        if accessor.is_none() && !auto && self.is(TokenKind::Star) {
            self.next()?;
            is_gen = true;
        }
        // Static methods and computed keys work on the constructor: `ctor proto` ->
        // `proto ctor` (a computed key goes on top of it).
        let early_swap = is_static && self.is(TokenKind::LBracket);
        if early_swap {
            self.code().swap();
        }
        let key_tok = self.tok;
        if key_tok.kind == TokenKind::LBracket
            && !has_decs
            && let (Some(i), Some(e)) = (first_modifier, &mut self.erased)
        {
            // ts-blank-space: `private [k]` must not join the line before (decorators
            // separate them).
            e[i].2 = super::ts::ER_SEMI;
        }
        let key = if key_tok.kind == TokenKind::PrivateName {
            let a = self.ident_atom()?;
            self.next()?;
            Key::Private(a)
        } else {
            match self.parse_property_key()? {
                Some(a) => Key::Atom(a),
                None => Key::Computed,
            }
        };
        self.ts_eat_marker(TokenKind::Question)?;
        if thunk.is_some() && matches!(key, Key::Private(_)) {
            // Legacy decorators cannot decorate private elements.
            return syntax(self.tok.start, "decorators are not valid here");
        }
        let is_method = self.is(TokenKind::LParen) || (self.ts && self.is(TokenKind::Lt));
        if auto && is_method {
            return syntax(self.tok.start, "expecting ';'");
        }
        let swapped = early_swap || (is_static && (is_method || auto));
        if swapped && !early_swap {
            self.code().swap();
        }
        let plain = !is_async && !is_gen && accessor.is_none();
        let ident_like =
            matches!(key_tok.kind, TokenKind::String) || key_tok.kind.is_identifier_name();
        if let Key::Atom(a) = key
            && ident_like
        {
            let text = self.heap.atom_name(a);
            if is_static && text == "constructor" && !is_method {
                return syntax(self.tok.start, "invalid field name");
            }
            if !is_static && text == "constructor" {
                if !is_method {
                    return syntax(self.tok.start, "invalid field name");
                }
                if !plain {
                    return syntax(self.tok.start, "invalid method name");
                }
                if has_decs {
                    return syntax(self.tok.start, "decorators are not valid here");
                }
                if b.has_ctor {
                    return syntax(
                        self.tok.start,
                        "property constructor appears more than once",
                    );
                }
                b.has_ctor = true;
                return self.parse_ctor(b);
            }
            if is_static && text == "prototype" {
                return syntax(self.tok.start, "invalid method name");
            }
        }
        if let Key::Private(a) = key
            && self.heap.atom_name(a) == "#constructor"
        {
            return syntax(self.tok.start, "invalid method name");
        }
        if is_method {
            let el = Elem {
                key,
                start,
                is_static,
                decs,
                thunk,
            };
            self.parse_class_method(b, el, accessor, (is_async, is_gen))?;
        } else {
            if !plain || self.is(TokenKind::LBrace) {
                return syntax(self.tok.start, "invalid property name");
            }
            self.parse_field(b, key, is_static, (decs, thunk), auto)?;
        }
        if swapped {
            self.code().swap();
        }
        Ok(())
    }

    /// A method, getter or setter after its key (computed keys are on the stack), with
    /// the target object (prototype or constructor) under it.
    fn parse_class_method(
        &mut self,
        b: &mut ClassBuild,
        el: Elem,
        accessor: Option<FuncKind>,
        flavor: (bool, bool),
    ) -> PResult<()> {
        let Elem {
            key,
            start,
            is_static,
            decs,
            thunk,
        } = el;
        let kind = accessor.unwrap_or(FuncKind::Method);
        let (prefix, word) = match kind {
            FuncKind::Getter => (1u8, "get "),
            FuncKind::Setter => (2, "set "),
            _ => (0, ""),
        };
        let fname = match key {
            Key::Atom(a) | Key::Private(a) if prefix == 0 => a,
            Key::Atom(a) | Key::Private(a) => {
                let n = crate::text::cat!(word, self.heap.atom_name(a));
                self.heap.atom_str(&n)?
            }
            Key::Computed => Atom::NULL,
        };
        if let Key::Private(a) = key
            && accessor.is_some()
        {
            // qjs reports duplicate accessors at the `(`.
            let pk = if kind == FuncKind::Getter {
                PrivKind::Getter
            } else {
                PrivKind::Setter
            };
            self.declare_private(a, pk, is_static, self.tok.start)?;
        }
        if let Some(decs) = decs {
            let dk = match kind {
                FuncKind::Getter => DEC_GETTER,
                FuncKind::Setter => DEC_SETTER,
                _ => DEC_METHOD,
            };
            let key = match key {
                Key::Atom(a) => DecKey::Atom(a),
                Key::Private(a) => DecKey::Private(a),
                Key::Computed => {
                    // The decorators need the key again.
                    let v = self.dec_hidden(b.scope, &mut b.hidden)?;
                    self.code()
                        .call_intrinsic(intrinsic_id_of!("ToPropertyKey"), 1);
                    self.code().dup();
                    let r = self.init_ref(v, start);
                    self.code().name_init(r);
                    DecKey::Computed(v)
                }
            };
            b.decs.push_out(DecElem {
                kind: dk,
                is_static,
                key,
                decs,
                rec: Atom::NULL,
            });
        }
        let dk = match kind {
            FuncKind::Getter => DEC_GETTER,
            FuncKind::Setter => DEC_SETTER,
            _ => DEC_METHOD,
        };
        // Legacy decorators: the key (a computed one is kept, as qjs: a parameter
        // decorator may need it) and the parameter decorators and types. (A private
        // method's parameter decorators get its name as a string key, as qjs.)
        let mut legacy = None;
        if b.legacy {
            let lkey = match key {
                Key::Computed => {
                    let v = self.dec_hidden(b.scope, &mut b.hidden)?;
                    self.code()
                        .call_intrinsic(intrinsic_id_of!("ToPropertyKey"), 1);
                    self.code().dup();
                    let r = self.init_ref(v, start);
                    self.code().name_init(r);
                    DecKey::Computed(v)
                }
                Key::Atom(a) | Key::Private(a) => DecKey::Atom(a),
            };
            legacy = Some((lkey, DecParams::new(thunk, b.metadata)));
        }
        let dp = legacy.as_mut().map(|(_, dp)| dp);
        let child = self.parse_method_function_dp(kind, fname, start, flavor, dp)?;
        if let Some((lkey, mut dp)) = legacy
            && let Some(mut t) = dp.thunk.take()
        {
            if b.metadata {
                let is_async = flavor == (true, false);
                t = self.thunk_method_metadata(t, dk, &dp, is_async)?;
            }
            b.legacy_elems.push_out(LegacyElem {
                kind: dk,
                is_static,
                key: lkey,
                thunk: t,
            });
        }
        self.code().fclosure(child);
        match key {
            Key::Atom(a) => self.code().define_class_method(a, prefix),
            Key::Computed => {
                self.code().set_function_name(prefix);
                self.code().define_class_method_elem(prefix);
            }
            Key::Private(a) => {
                if accessor.is_none() {
                    // qjs reports a duplicate method after its body.
                    self.declare_private(a, PrivKind::Method, is_static, self.tok.start)?;
                }
                self.install_private(b, a, kind, is_static, start)?;
            }
        }
        Ok(())
    }

    /// The closure of private method, getter or setter `a` on the stack (its home
    /// object under it): it goes into a hidden binding until the initializer installs
    /// it.
    fn install_private(
        &mut self,
        b: &mut ClassBuild,
        a: Atom,
        kind: FuncKind,
        is_static: bool,
        start: u32,
    ) -> PResult<()> {
        let word = match kind {
            FuncKind::Getter => "get ",
            FuncKind::Setter => "set ",
            _ => "",
        };
        let hidden = self.private_closure_atom(word, a)?;
        let v = self.f.add_var(hidden, VarKind::Hidden, b.scope);
        self.code().set_home_object();
        let r = self.init_ref(hidden, start);
        self.code().name_init(r);
        // The initializer function installs it.
        let has_init = if is_static {
            b.stat.is_some()
        } else {
            b.inst.is_some()
        };
        if !has_init {
            let init = self.new_init(b.start)?;
            if is_static {
                b.stat = Some(init);
            } else {
                b.inst = Some(init);
            }
        }
        let list = if is_static {
            &mut b.stat_methods
        } else {
            &mut b.inst_methods
        };
        match list.iter_mut().find(|m| m.name == a) {
            Some(m) => {
                m.kind = PrivKind::Accessor;
                if kind == FuncKind::Getter {
                    m.get = Some(v);
                } else {
                    m.set = Some(v);
                }
            }
            None => list.push_out(Install {
                name: a,
                kind: match kind {
                    FuncKind::Getter | FuncKind::Setter => PrivKind::Accessor,
                    _ => PrivKind::Method,
                },
                get: (kind != FuncKind::Setter).then_some(v),
                set: (kind == FuncKind::Setter).then_some(v),
            }),
        }
        Ok(())
    }

    /// Declares private name `name`: a getter and a setter (both static or both not)
    /// may share one; anything else is a duplicate.
    fn declare_private(
        &mut self,
        name: Atom,
        kind: PrivKind,
        is_static: bool,
        pos: u32,
    ) -> PResult<()> {
        let ctx = self.classes.last_mut().expect("class context");
        if let Some(p) = ctx.privates.iter_mut().find(|p| p.name == name) {
            let pair = matches!(
                (p.kind, kind),
                (PrivKind::Getter, PrivKind::Setter) | (PrivKind::Setter, PrivKind::Getter)
            );
            if !pair || p.is_static != is_static {
                return syntax(pos, "private class field is already defined");
            }
            p.kind = PrivKind::Accessor;
            if let Some(v) = self.f.find_in_scope(self.scope, name) {
                self.f.vars[v as usize].kind = VarKind::PrivateAccessor;
            }
            return Ok(());
        }
        ctx.privates.push_out(Private {
            name,
            kind,
            is_static,
        });
        let vk = match kind {
            PrivKind::Field => VarKind::PrivateName,
            PrivKind::Method | PrivKind::Getter => VarKind::PrivateMethod,
            _ => VarKind::PrivateAccessor,
        };
        let scope = self.scope;
        self.f.add_var(name, vk, scope);
        Ok(())
    }

    /// A field after its key: the initializer goes into the instance or static
    /// initializer function. A computed key is converted now and kept in a hidden
    /// binding. An auto-accessor (`auto`) is a field of a private storage name and a
    /// getter and setter of it. `decs` are its decorators: standard or legacy.
    fn parse_field(
        &mut self,
        b: &mut ClassBuild,
        key: Key,
        is_static: bool,
        (decs, mut thunk): (Option<Atom>, Option<Thunk>),
        auto: bool,
    ) -> PResult<()> {
        self.ts_eat_marker(TokenKind::Bang)?;
        if b.metadata
            && let Some(t) = thunk.take()
        {
            thunk = Some(self.thunk_field_metadata(t)?);
        }
        self.ts_annotation()?;
        let pos = self.tok.start;
        // The hidden bindings in qjs's order (the storage name shows in messages).
        let rec = match decs {
            Some(_) => Some(self.dec_hidden(b.scope, &mut b.hidden)?),
            None => None,
        };
        let key_var = match key {
            Key::Computed => {
                self.code()
                    .call_intrinsic(intrinsic_id_of!("ToPropertyKey"), 1);
                let atom = if auto {
                    self.dec_hidden(b.scope, &mut b.hidden)?
                } else {
                    let atom = self
                        .heap
                        .atom_str(&crate::text::cat!("<key ", b.keys, ">"))?;
                    b.keys += 1;
                    self.f.add_var(atom, VarKind::Hidden, b.scope);
                    atom
                };
                let r = self.init_ref(atom, pos);
                self.code().name_init(r);
                Some(atom)
            }
            Key::Private(a) => {
                let kind = if auto {
                    PrivKind::Accessor
                } else {
                    PrivKind::Field
                };
                self.declare_private(a, kind, is_static, pos)?;
                None
            }
            Key::Atom(_) => None,
        };
        let storage = if auto {
            let storage = self.dec_hidden(b.scope, &mut b.hidden)?;
            let v = self.f.vars.len() - 1;
            self.f.vars[v].kind = VarKind::PrivateName;
            self.emit_auto_accessor(b, key, key_var, storage, is_static, pos)?;
            Some(storage)
        } else {
            None
        };
        let rec = match (decs, rec) {
            (Some(decs), Some(rec)) => {
                b.decs.push_out(DecElem {
                    kind: if auto { DEC_ACCESSOR } else { DEC_FIELD },
                    is_static,
                    key: match key {
                        Key::Atom(a) => DecKey::Atom(a),
                        Key::Private(a) => DecKey::Private(a),
                        Key::Computed => DecKey::Computed(key_var.expect("key binding")),
                    },
                    decs,
                    rec,
                });
                Some(rec)
            }
            _ => None,
        };
        if let Some(t) = thunk {
            b.legacy_elems.push_out(LegacyElem {
                kind: if auto { DEC_ACCESSOR } else { DEC_FIELD },
                is_static,
                key: match key {
                    Key::Atom(a) | Key::Private(a) => DecKey::Atom(a),
                    Key::Computed => DecKey::Computed(key_var.expect("key binding")),
                },
                thunk: t,
            });
        }
        let init = match if is_static {
            b.stat.take()
        } else {
            b.inst.take()
        } {
            Some(i) => i,
            None => self.new_init(b.start)?,
        };
        self.resume_function(*init.f);
        self.emit_this(pos);
        match (storage, key) {
            (Some(s), _) => {
                let r = self.name_ref(s, pos);
                self.code().name_get(r);
            }
            (None, Key::Atom(a)) => self.code().push_atom(a),
            (None, Key::Private(a)) => {
                let r = self.name_ref(a, pos);
                self.code().name_get(r);
            }
            (None, Key::Computed) => {
                let r = self.name_ref(key_var.expect("key binding"), pos);
                self.code().name_get(r);
            }
        }
        let eq = self.tok.start;
        if self.eat(TokenKind::Assign)? {
            let value_start = self.tok.start;
            match key {
                Key::Atom(a) | Key::Private(a) => {
                    self.parse_named(a, |p| p.parse_assign(false))?;
                }
                _ => {
                    self.parse_assign(false)?;
                }
            }
            if key == Key::Computed
                && !auto
                && self.last_anon_fn == Some((value_start, self.prev_end))
            {
                self.code().set_function_name(0);
            }
            if is_static {
                let kind = if storage.is_some() || matches!(key, Key::Private(_)) {
                    ClassSpanKind::StaticPrivateInit
                } else {
                    ClassSpanKind::StaticInit
                };
                self.class_span(eq, self.prev_end, kind);
            }
        } else {
            self.code().undefined();
        }
        if let Some(rec) = rec {
            self.emit_run_inits(rec, pos);
        }
        self.mark(pos);
        if storage.is_some() || matches!(key, Key::Private(_)) {
            self.code().define_private_field(0);
        } else {
            self.code().define_class_field();
        }
        self.code().drop();
        if let Some(rec) = rec {
            self.emit_run_rec_extras(rec, pos);
        }
        let f = alloc::boxed::Box::new(self.park_function()?);
        let init = Init {
            f,
            body: init.body,
            methods: init.methods,
        };
        if is_static {
            b.stat = Some(init);
        } else {
            b.inst = Some(init);
        }
        self.expect_semicolon()
    }

    /// The getter and setter of auto-accessor `key` on private name `storage`, with the
    /// target object (prototype or constructor) on the stack.
    fn emit_auto_accessor(
        &mut self,
        b: &mut ClassBuild,
        key: Key,
        key_var: Option<Atom>,
        storage: Atom,
        is_static: bool,
        pos: u32,
    ) -> PResult<()> {
        for (synth, kind, prefix) in [
            (Synth::AccessorGet, FuncKind::Getter, 1u8),
            (Synth::AccessorSet, FuncKind::Setter, 2),
        ] {
            let word = if prefix == 1 { "get " } else { "set " };
            match key {
                Key::Atom(a) => {
                    let n = crate::text::cat!(word, self.heap.atom_name(a));
                    let fname = self.heap.atom_str(&n)?;
                    self.emit_synth_fn(synth, storage, fname, pos)?;
                    self.code().define_class_method(a, prefix);
                }
                Key::Computed => {
                    let r = self.name_ref(key_var.expect("key binding"), pos);
                    self.code().name_get(r);
                    self.emit_synth_fn(synth, storage, Atom::NULL, pos)?;
                    self.code().set_function_name(prefix);
                    self.code().define_class_method_elem(prefix);
                }
                Key::Private(a) => {
                    let n = crate::text::cat!(word, self.heap.atom_name(a));
                    let fname = self.heap.atom_str(&n)?;
                    self.emit_synth_fn(synth, storage, fname, pos)?;
                    self.install_private(b, a, kind, is_static, pos)?;
                }
            }
        }
        Ok(())
    }

    /// A new parked field initializer function (`FieldInit`): it starts by jumping to
    /// the private method installation, which `finish_init` emits at its end.
    fn new_init(&mut self, start: u32) -> PResult<Init> {
        self.begin_function(FuncKind::FieldInit)?;
        self.f.src_start = start;
        let body = self.new_label();
        let methods = self.new_label();
        self.code().jump(methods);
        self.place(body);
        let f = alloc::boxed::Box::new(self.park_function()?);
        Ok(Init { f, body, methods })
    }

    /// Ends a field initializer function: returns, then the private method
    /// installation (jumped to at the start), then back to the initializers. Returns
    /// its child index.
    fn finish_init(
        &mut self,
        init: Init,
        methods: &[Install],
        extras: Option<Atom>,
    ) -> PResult<u32> {
        let pos = self.prev_end;
        self.resume_function(*init.f);
        self.f.src_end = pos;
        self.code().return_undef();
        self.place(init.methods);
        for m in methods {
            self.emit_this(pos);
            let r = self.name_ref(m.name, pos);
            self.code().name_get(r);
            let names: Vec<Atom> = [m.get, m.set]
                .iter()
                .map(|v| {
                    v.map_or(Atom::NULL, |v| {
                        self.stack.last().unwrap().f.vars[v as usize].name
                    })
                })
                .collect();
            let load = |p: &mut Self, a: Atom| {
                if a.is_null() {
                    p.code().undefined();
                } else {
                    let r = p.name_ref(a, pos);
                    p.code().name_get(r);
                }
            };
            if m.kind == PrivKind::Method {
                load(self, names[0]);
                self.code().define_private_field(1);
            } else {
                load(self, names[0]);
                load(self, names[1]);
                self.code().define_private_accessor();
            }
            self.code().drop();
        }
        if let Some(state) = extras {
            self.emit_instance_extras(state, pos);
        }
        self.code().jump(init.body);
        self.end_function()
    }

    /// Records a span of the outermost class's code that its evaluation runs
    /// (`compiler::class_spans`).
    fn class_span(&mut self, start: u32, end: u32, kind: ClassSpanKind) {
        if self.class_depth == 1
            && start < end
            && let Some(spans) = &mut self.class_spans
        {
            spans.push_out(ClassSpan { start, end, kind });
        }
    }

    /// `static { ... }` after `static`: a function called by the static initializer
    /// with `this` = the class.
    fn parse_static_block(&mut self, b: &mut ClassBuild) -> PResult<()> {
        let start = self.tok.start;
        let init = match b.stat.take() {
            Some(i) => i,
            None => self.new_init(b.start)?,
        };
        self.resume_function(*init.f);
        self.emit_this(start);
        self.enter()?;
        self.begin_function(FuncKind::StaticBlock)?;
        self.f.src_start = start;
        self.parse_function_body()?;
        self.f.src_end = self.prev_end;
        self.class_span(start + 1, self.prev_end - 1, ClassSpanKind::StaticBlock);
        let child = self.end_function()?;
        self.leave();
        self.code().fclosure(child);
        self.code().set_home_object();
        self.code().swap();
        self.mark(start);
        self.code().call(0);
        self.code().drop();
        let f = alloc::boxed::Box::new(self.park_function()?);
        b.stat = Some(Init {
            f,
            body: init.body,
            methods: init.methods,
        });
        Ok(())
    }

    /// `constructor(params) { body }` after the key: the reserved child.
    fn parse_ctor(&mut self, b: &mut ClassBuild) -> PResult<()> {
        self.enter()?;
        let props = if self.ts {
            self.ts_scan_param_props()?
        } else {
            Vec::new()
        };
        self.begin_ctor(b, props)?;
        let params = self.tok.start;
        self.expect(TokenKind::LParen, "(")?;
        if b.legacy {
            // Its parameter decorators and types belong to the class.
            let mut dp = DecParams::new(b.class_thunk.take(), b.metadata);
            self.parse_params_dp(Some(&mut dp))?;
            b.class_thunk = dp.thunk;
            b.ctor_types = dp.types;
        } else {
            self.parse_params()?;
        }
        self.syn_part(super::smap_syntax::Role::Params, params, self.prev_end);
        if !b.derived {
            // Parameter properties are assigned once the parameters are (a derived
            // constructor does it after `super()`).
            let pos = self.prev_end;
            self.emit_this(pos);
            self.emit_param_prop_assigns(pos);
            self.code().drop();
        }
        self.parse_function_body()?;
        self.emit_ctor_epilogue();
        self.f.src_end = self.prev_end;
        self.end_function_at(Some(b.ctor_slot))?;
        self.leave();
        Ok(())
    }

    /// Starts a class constructor: the `new` check, and for a base class the field
    /// initializer, which runs before the parameters (OrdinaryCallEvaluateBody after
    /// InitializeInstanceElements, 10.2.2).
    fn begin_ctor(&mut self, b: &ClassBuild, props: Vec<Atom>) -> PResult<()> {
        let kind = if b.derived {
            FuncKind::DerivedCtor
        } else {
            FuncKind::ClassCtor
        };
        let pos = self.tok.start;
        self.begin_function(kind)?;
        self.f.name = b.name;
        self.f.fields_atom = b.fields_atom;
        self.f.param_props = props;
        self.f.src_start = b.start;
        self.mark(pos);
        self.code().check_ctor();
        if b.derived {
            let ret = self.heap.atom_str("<return>")?;
            self.f
                .add_var(ret, VarKind::Internal, super::func::BODY_SCOPE);
            let l = self.new_label();
            self.f.ctor_return = Some((l, ret));
        } else {
            self.emit_this(pos);
            self.emit_param_prop_decls(pos);
            self.emit_init_fields(b.fields_atom, pos);
            self.code().drop();
        }
        Ok(())
    }

    /// `this` on the stack (it stays): defines the fields of the constructor's
    /// parameter properties as `undefined`, before the other fields
    /// (`useDefineForClassFields`, as tsc and dquickts).
    pub(super) fn emit_param_prop_decls(&mut self, pos: u32) {
        let props = self.ctor_param_props();
        for a in props {
            self.code().dup();
            self.code().push_atom(a);
            self.code().undefined();
            self.mark(pos);
            self.code().define_class_field();
            self.code().drop();
        }
    }

    /// `this` on the stack (it stays): `this.x = x` for the parameter properties.
    pub(super) fn emit_param_prop_assigns(&mut self, pos: u32) {
        let props = self.ctor_param_props();
        for a in props {
            self.code().dup();
            let r = self.name_ref(a, pos);
            self.code().name_get(r);
            self.mark(pos);
            self.code().put_field(a, 0);
        }
    }

    /// The parameter properties of the constructor that owns `this` here.
    fn ctor_param_props(&self) -> Vec<Atom> {
        match self.own_this_function() {
            Ok(None) => self.f.param_props.clone(),
            Ok(Some(i)) => self.stack[i].f.param_props.clone(),
            Err(()) => Vec::new(),
        }
    }

    /// The default constructor: `constructor() {}`, or for a derived class
    /// `constructor(...args) { super(...args) }` (without iterating `args`).
    fn default_ctor(&mut self, b: &ClassBuild) -> PResult<()> {
        let pos = b.start;
        self.begin_ctor(b, Vec::new())?;
        if b.derived {
            self.declare_pseudo(VarKind::ActiveFunc);
            let active = self.atom_active;
            let r = self.name_ref(active, pos);
            self.code().name_get(r);
            self.code().get_super();
            self.code().rest(0);
            let nt = self
                .new_target_ref(pos)
                .ok_or_else(|| super::resolve::internal("no new.target"))?;
            self.code().name_get(nt);
            self.mark(pos);
            self.code().super_call_spread();
            self.emit_bind_this(pos);
            self.code().return_();
            self.f.ctor_return = None;
        } else {
            self.code().return_undef();
        }
        self.end_function_at(Some(b.ctor_slot))?;
        Ok(())
    }
}
