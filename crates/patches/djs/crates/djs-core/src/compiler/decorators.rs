//! Decorators (TC39 2023-11, 0065, D1650, docs/compiler.md 7.11), after qjs.
//!
//! A decorator list evaluates to an array of decorator and receiver pairs (`@a.b`
//! calls `b` with `this` = `a`). A class element's list goes into a hidden binding
//! before the element's key is evaluated; the class's list is evaluated before `class`
//! (`Parser::class_decs`). Once the body is defined (`ctor proto` on the stack), the
//! class's decoration state is created and the elements are decorated in qjs's order:
//! static methods and accessors, instance ones, static fields, instance fields, each by
//! `%ApplyDecorators` (builtins/decorators.ts). A public method, getter, setter or
//! auto-accessor is read from its home object and redefined there; a private one's
//! closures are hidden bindings (compiler/class.rs) that the result replaces before the
//! initializers install them. Fields and auto-accessors get a Record whose initializers
//! the field initializer runs on the value, and whose extra initializers it runs after
//! the definition. The class decorators come last, after the instance field
//! initializer exists and before the inner binding, the static extra initializers, the
//! static initializer (`this` = the new class) and the class extra initializers.
//!
//! The access object of a private element (`context.access`) has synthesized arrows
//! `(o) => o.#x`, `(o, v) => { o.#x = v; }` and `(o) => #x in o`; an auto-accessor
//! `accessor x` is a private storage name and a synthesized getter and setter
//! `get x() { return this.<storage>; }` (`emit_synth_fn`).
//!
//! TypeScript legacy decorators (`experimentalDecorators`, D1651) are the second half:
//! thunks evaluated after the class (`Thunk`), parameter decorators (`DecParams`) and
//! tsc's `__decorate` (`emit_legacy_apply`); the metadata types are in ts_meta.rs.
use crate::PushOut;
use crate::bytecode::intrinsics::intrinsic_id_of;

use alloc::vec::Vec;

use super::PResult;
use super::func::{FuncKind, FuncState, ScopeId, VarKind};
use super::parser::{Parser, syntax};
use super::ts_meta::{MetaKind, TsMeta};
use crate::heap::Atom;
use crate::lexer::{Kw, TokenKind};

/// Element kinds and flags of `%ApplyDecorators` (builtins/decorators.ts).
pub(super) const DEC_CLASS: u8 = 0;
pub(super) const DEC_METHOD: u8 = 1;
pub(super) const DEC_GETTER: u8 = 2;
pub(super) const DEC_SETTER: u8 = 3;
pub(super) const DEC_ACCESSOR: u8 = 4;
pub(super) const DEC_FIELD: u8 = 5;
const DEC_STATIC: u8 = 8;
const DEC_PRIVATE: u8 = 16;

/// Slots of the decoration state and of an element Record.
const STATE_STATIC_EXTRAS: i8 = 1;
const STATE_INSTANCE_EXTRAS: i8 = 2;
const STATE_CLASS_EXTRAS: i8 = 3;
const REC_EXTRAS: i8 = 1;
const REC_GET: i8 = 2;
const REC_SET: i8 = 3;

/// The key of a decorated element.
#[derive(Clone, Copy)]
pub(super) enum DecKey {
    Atom(Atom),
    /// The hidden binding of the converted key.
    Computed(Atom),
    /// The private name `#x`.
    Private(Atom),
}

/// A decorated class element.
pub(super) struct DecElem {
    pub kind: u8,
    pub is_static: bool,
    pub key: DecKey,
    /// The hidden binding of the decorator array.
    pub decs: Atom,
    /// Fields and auto-accessors: the hidden binding of their Record.
    pub rec: Atom,
}

/// TypeScript legacy decorators (`experimentalDecorators`): an arrow function,
/// parked while the class is parsed, whose body evaluates the decorators of one
/// element (or of the class) and returns their array (qjs's DecThunk). It is created
/// and called in the class's scope after the class is defined, as tsc evaluates them.
pub(super) struct Thunk {
    /// Boxed: thunks sit by value in the class parser's locals, and a FuncState inline
    /// made a nested class level ~7 KiB of wasm stack (D1941).
    pub f: alloc::boxed::Box<FuncState>,
    /// The decorators pushed so far.
    pub count: u32,
    /// The first `@`.
    pub start: u32,
}

/// Legacy decorators: the parameter decorators and types of the method or
/// constructor being parsed. Its parameter decorators go into `thunk` (the method's,
/// or for the constructor the class's), created if needed.
pub(super) struct DecParams {
    pub thunk: Option<Thunk>,
    /// Collect the types (`emitDecoratorMetadata`).
    pub metadata: bool,
    pub types: Vec<TsMeta>,
    pub ret: Option<TsMeta>,
}

impl DecParams {
    pub fn new(thunk: Option<Thunk>, metadata: bool) -> DecParams {
        DecParams {
            thunk,
            metadata,
            types: Vec::new(),
            ret: None,
        }
    }
}

/// A class element with legacy decorators.
pub(super) struct LegacyElem {
    pub kind: u8,
    pub is_static: bool,
    /// `Atom` or `Computed` (private elements cannot have them).
    pub key: DecKey,
    pub thunk: Thunk,
}

/// A synthesized function (`emit_synth_fn`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Synth {
    /// `get x() { return this.#s; }`
    AccessorGet,
    /// `set x(v) { this.#s = v; }`
    AccessorSet,
    /// `(o) => o.#x`
    AccessGet,
    /// `(o, v) => { o.#x = v; }`
    AccessSet,
    /// `(o) => #x in o`
    AccessHas,
}

impl Parser<'_, '_> {
    /// One or more decorators at `@`: pushes the array of decorator and receiver pairs.
    pub(super) fn parse_decorators(&mut self) -> PResult<()> {
        // The slim build has no decorators (D2411).
        if !crate::FULL {
            return syntax(self.tok.start, "decorators are not available in this build");
        }
        let mut n: u32 = 0;
        while self.is(TokenKind::At) {
            self.parse_decorator(true)?;
            n += 2;
            if n > u16::MAX as u32 {
                return syntax(self.tok.start, "too many decorators");
            }
        }
        self.code().array_from(n as u16);
        Ok(())
    }

    /// `@` DecoratorMemberExpression, DecoratorCallExpression or `( Expression )`, as
    /// qjs parses it: a primary expression and then `.x`, `.#x` and calls (no `[`,
    /// `?.` or template). Pushes the decorator and, with `receiver`, its receiver.
    fn parse_decorator(&mut self, receiver: bool) -> PResult<()> {
        self.enter()?;
        let at = self.tok.start;
        self.next()?;
        let mut e = match self.tok.kind {
            TokenKind::Ident => self.parse_ident_ref()?,
            TokenKind::LParen => self.parse_primary()?,
            _ => return syntax(at, "invalid decorator"),
        };
        loop {
            let tok = self.tok;
            match tok.kind {
                TokenKind::Dot => {
                    self.next()?;
                    e = self.parse_dot_name(tok.start)?;
                }
                TokenKind::LParen => {
                    self.prepare_call(e, at);
                    let args = self.parse_args()?;
                    self.mark(tok.start);
                    self.emit_call(args);
                    e = super::expr::Expr::Value;
                }
                _ => break,
            }
        }
        if receiver {
            // `dec recv`, like a call's callee and `this`.
            self.prepare_call(e, at);
            // They stay for the class: its suspensions are not described (D2563).
            self.names_pop(2);
        }
        self.leave();
        Ok(())
    }

    /// A statement starting with `@`: the decorators of a class declaration (or, in
    /// strip-types, of `export class`).
    pub(super) fn parse_decorated_statement(&mut self) -> PResult<()> {
        let start = self.tok.start;
        self.parse_class_decorators()?;
        self.expect_decorated_class(true)?;
        self.parse_statement_list_item()?;
        if self.class_decs.take().is_some() || self.class_thunk.take().is_some() {
            return syntax(start, "decorators must be followed by a class");
        }
        Ok(())
    }

    /// The decorators of the next class (`parse_class` takes them): their array on the
    /// stack (`class_decs`), or with legacy decorators their thunk (`class_thunk`).
    pub(super) fn parse_class_decorators(&mut self) -> PResult<()> {
        let start = self.tok.start;
        if self.legacy_decorators() {
            let t = self.parse_legacy_decorators(None)?;
            self.class_thunk = Some(t);
        } else {
            self.parse_decorators()?;
            self.class_decs = Some(start);
        }
        Ok(())
    }

    /// After decorators: the current token must start the decorated class.
    pub(super) fn expect_decorated_class(&mut self, allow_export: bool) -> PResult<()> {
        let ok = self.is(TokenKind::Class)
            || (allow_export && self.strip && self.is(TokenKind::Export))
            || (allow_export
                && self.ts
                && self.tok.is_contextual(Kw::Abstract)
                && self.peek().kind == TokenKind::Class);
        if !ok {
            return syntax(self.tok.start, "decorators must be followed by a class");
        }
        Ok(())
    }

    /// A new hidden binding `<dec>N` of the class body scope, numbered like qjs's (an
    /// auto-accessor's storage name shows in its messages).
    pub(super) fn dec_hidden(&mut self, scope: ScopeId, n: &mut u32) -> PResult<Atom> {
        let atom = self.heap.atom_str(&crate::text::cat!("<dec>", n))?;
        *n += 1;
        self.f.add_var(atom, VarKind::Hidden, scope);
        Ok(atom)
    }

    /// Emits the closure of a synthesized function on the private name `name`.
    /// Accessors get the function name `fname`.
    pub(super) fn emit_synth_fn(
        &mut self,
        kind: Synth,
        name: Atom,
        fname: Atom,
        pos: u32,
    ) -> PResult<()> {
        let fk = match kind {
            Synth::AccessorGet => FuncKind::Getter,
            Synth::AccessorSet => FuncKind::Setter,
            _ => FuncKind::Arrow,
        };
        self.begin_function(fk)?;
        self.f.name = fname;
        self.f.src_start = pos;
        self.f.src_end = pos;
        let params = match kind {
            Synth::AccessorGet => 0,
            Synth::AccessorSet | Synth::AccessGet | Synth::AccessHas => 1,
            Synth::AccessSet => 2,
        };
        for i in 0..params {
            let a = self.heap.atom_str(&crate::text::cat!("<arg ", i, ">"))?;
            let v = self.declare_param(a, pos)?;
            self.f.vars[v as usize].init_end = 0;
            self.f.length += 1;
        }
        let method = matches!(kind, Synth::AccessorGet | Synth::AccessorSet);
        if kind == Synth::AccessHas {
            let r = self.name_ref(name, pos);
            self.code().name_get(r);
            self.code().get_arg(0);
            self.mark(pos);
            self.code().private_in();
            self.code().return_();
        } else {
            if method {
                self.emit_this(pos);
            } else {
                self.code().get_arg(0);
            }
            let r = self.name_ref(name, pos);
            self.code().name_get(r);
            self.mark(pos);
            match kind {
                Synth::AccessorGet | Synth::AccessGet => {
                    self.code().get_private_field();
                    self.code().return_();
                }
                _ => {
                    self.code().get_arg(params - 1);
                    self.mark(pos);
                    self.code().put_private_field();
                    self.code().return_undef();
                }
            }
        }
        let child = self.end_function()?;
        self.code().fclosure(child);
        Ok(())
    }

    /// The access object of a private element of kind `kind`.
    fn emit_private_access(&mut self, kind: u8, name: Atom, pos: u32) -> PResult<()> {
        self.code().object();
        if kind != DEC_SETTER {
            self.emit_synth_fn(Synth::AccessGet, name, Atom::NULL, pos)?;
            let a = self.heap.atom_str("get")?;
            self.code().define_field(a, 0);
        }
        if matches!(kind, DEC_SETTER | DEC_FIELD | DEC_ACCESSOR) {
            self.emit_synth_fn(Synth::AccessSet, name, Atom::NULL, pos)?;
            let a = self.heap.atom_str("set")?;
            self.code().define_field(a, 0);
        }
        self.emit_synth_fn(Synth::AccessHas, name, Atom::NULL, pos)?;
        let a = self.heap.atom_str("has")?;
        self.code().define_field(a, 0);
        Ok(())
    }

    /// The hidden binding of a private method's closure (compiler/class.rs).
    pub(super) fn private_closure_atom(&mut self, word: &str, name: Atom) -> PResult<Atom> {
        let hidden = crate::text::cat!("<", word, self.heap.atom_name(name), ">");
        Ok(self.heap.atom_str(&hidden)?)
    }

    /// With `ctor proto` on the stack: creates the decoration state (in the hidden
    /// binding `state`) and decorates the elements.
    pub(super) fn emit_decorate_elements(
        &mut self,
        elems: &[DecElem],
        state: Atom,
        derived: bool,
        pos: u32,
    ) -> PResult<()> {
        self.code().pick(1);
        if derived {
            self.code().push_true();
        } else {
            self.code().push_false();
        }
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("DecoratorStart"), 2);
        let r = self.init_ref(state, pos);
        self.code().name_init(r);
        for pass in 0..4 {
            for d in elems {
                if d.is_static != (pass & 1 == 0) || (d.kind == DEC_FIELD) != (pass >= 2) {
                    continue;
                }
                self.emit_decorate(d, state, pos)?;
            }
        }
        Ok(())
    }

    /// Decorates one element (`ctor proto` on the stack).
    fn emit_decorate(&mut self, d: &DecElem, state: Atom, pos: u32) -> PResult<()> {
        let (word_get, word_set) = ("get ", "set ");
        // Target.
        match d.key {
            DecKey::Private(a) => self.emit_private_access(d.kind, a, pos)?,
            _ if d.is_static => self.code().pick(1),
            _ => self.code().dup(),
        }
        // Key.
        match d.key {
            DecKey::Atom(a) | DecKey::Private(a) => self.code().push_atom(a),
            DecKey::Computed(v) => {
                let r = self.name_ref(v, pos);
                self.code().name_get(r);
            }
        }
        // Value.
        let mut hidden = Atom::NULL;
        let mut hidden_set = Atom::NULL;
        match (d.key, d.kind) {
            (DecKey::Private(a), DEC_METHOD) => hidden = self.private_closure_atom("", a)?,
            (DecKey::Private(a), DEC_GETTER) => hidden = self.private_closure_atom(word_get, a)?,
            (DecKey::Private(a), DEC_SETTER) => hidden = self.private_closure_atom(word_set, a)?,
            (DecKey::Private(a), DEC_ACCESSOR) => {
                hidden = self.private_closure_atom(word_get, a)?;
                hidden_set = self.private_closure_atom(word_set, a)?;
            }
            _ => {}
        }
        if hidden.is_null() {
            self.code().undefined();
        } else if hidden_set.is_null() {
            let r = self.name_ref(hidden, pos);
            self.code().name_get(r);
        } else {
            self.code().object();
            for (h, w) in [(hidden, "get"), (hidden_set, "set")] {
                let r = self.name_ref(h, pos);
                self.code().name_get(r);
                let a = self.heap.atom_str(w)?;
                self.code().define_field(a, 0);
            }
        }
        let r = self.name_ref(d.decs, pos);
        self.code().name_get(r);
        let r = self.name_ref(state, pos);
        self.code().name_get(r);
        let mut flags = d.kind;
        if d.is_static {
            flags |= DEC_STATIC;
        }
        if matches!(d.key, DecKey::Private(_)) {
            flags |= DEC_PRIVATE;
        }
        self.code().push_i8(flags as i8);
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("ApplyDecorators"), 6);
        // Result.
        if d.kind == DEC_FIELD || d.kind == DEC_ACCESSOR {
            if !hidden_set.is_null() {
                for (h, slot) in [(hidden, REC_GET), (hidden_set, REC_SET)] {
                    self.code().dup();
                    self.code().push_i8(slot);
                    self.code().call_intrinsic(intrinsic_id_of!("RecordGet"), 2);
                    let r = self.init_ref(h, pos);
                    self.code().name_init(r);
                }
            }
            let r = self.init_ref(d.rec, pos);
            self.code().name_init(r);
        } else if !hidden.is_null() {
            let r = self.init_ref(hidden, pos);
            self.code().name_init(r);
        } else {
            self.code().drop();
        }
        Ok(())
    }

    /// With the class on the stack (it stays): applies the class decorators (in the
    /// hidden binding `decs`, if any), leaving `class new_class`.
    pub(super) fn emit_decorate_class(
        &mut self,
        decs: Option<Atom>,
        name: Atom,
        state: Atom,
        pos: u32,
    ) -> PResult<()> {
        self.code().dup();
        if name.is_null() {
            self.code().undefined();
        } else {
            self.code().push_atom(name);
        }
        self.code().undefined();
        match decs {
            Some(d) => {
                let r = self.name_ref(d, pos);
                self.code().name_get(r);
            }
            None => self.code().undefined(),
        }
        let r = self.name_ref(state, pos);
        self.code().name_get(r);
        self.code().push_i8(DEC_CLASS as i8);
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("ApplyDecorators"), 6);
        Ok(())
    }

    /// Runs the extra initializers in slot `slot` of the state with `this` = the value
    /// on the top of the stack (it stays).
    pub(super) fn emit_state_extras(&mut self, state: Atom, slot: u8, pos: u32) {
        let r = self.name_ref(state, pos);
        self.code().name_get(r);
        self.code().push_i8(slot as i8);
        self.code().pick(2);
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("DecoratorRunExtras"), 3);
        self.code().drop();
    }

    /// In a field initializer, the field value on the stack: runs the initializers of
    /// the element Record `rec` on it.
    pub(super) fn emit_run_inits(&mut self, rec: Atom, pos: u32) {
        let r = self.name_ref(rec, pos);
        self.code().name_get(r);
        self.emit_this(pos);
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("DecoratorRunInits"), 3);
    }

    /// In a field initializer, after the definition: the extra initializers of `rec`.
    pub(super) fn emit_run_rec_extras(&mut self, rec: Atom, pos: u32) {
        let r = self.name_ref(rec, pos);
        self.code().name_get(r);
        self.code().push_i8(REC_EXTRAS);
        self.emit_this(pos);
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("DecoratorRunExtras"), 3);
        self.code().drop();
    }

    /// In the instance field initializer, after the private methods: the instance
    /// method extra initializers.
    pub(super) fn emit_instance_extras(&mut self, state: Atom, pos: u32) {
        let r = self.name_ref(state, pos);
        self.code().name_get(r);
        self.code().push_i8(STATE_INSTANCE_EXTRAS);
        self.emit_this(pos);
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("DecoratorRunExtras"), 3);
        self.code().drop();
    }

    // ---- TypeScript legacy decorators (0065 part 2) -----------------------------------

    /// TypeScript's `experimentalDecorators` are on (not in strip-types, which keeps
    /// decorators as they are).
    pub(super) fn legacy_decorators(&self) -> bool {
        self.ts && !self.strip && self.opts.experimental_decorators
    }

    /// `emitDecoratorMetadata` is on.
    pub(super) fn decorator_metadata(&self) -> bool {
        self.legacy_decorators() && self.opts.emit_decorator_metadata
    }

    /// A new empty thunk in the current function and scope.
    fn new_thunk(&mut self, start: u32) -> PResult<Thunk> {
        self.begin_function(FuncKind::Arrow)?;
        self.f.src_start = start;
        self.f.src_end = start;
        let f = self.park_function()?;
        Ok(Thunk {
            f: alloc::boxed::Box::new(f),
            count: 0,
            start,
        })
    }

    /// Legacy decorators at `@`: their expressions go into `thunk` (a new one if
    /// `None`).
    pub(super) fn parse_legacy_decorators(&mut self, thunk: Option<Thunk>) -> PResult<Thunk> {
        // The slim build has no decorators (D2411).
        if !crate::FULL {
            return syntax(self.tok.start, "decorators are not available in this build");
        }
        let mut t = match thunk {
            Some(t) => t,
            None => self.new_thunk(self.tok.start)?,
        };
        self.resume_function(*t.f);
        while self.is(TokenKind::At) {
            self.parse_decorator(false)?;
            t.count += 1;
        }
        *t.f = self.park_function()?;
        Ok(t)
    }

    /// Parameter decorators at `@` (after a parameter's `this` check, before its
    /// modifiers): `__param(index, dec)` decorators of the method, in its thunk. Only
    /// class methods and constructors with legacy decorators have `dp`.
    pub(super) fn parse_param_decorators(&mut self, dp: Option<&mut DecParams>) -> PResult<()> {
        // The slim build has no decorators (D2411).
        if !crate::FULL {
            return syntax(self.tok.start, "decorators are not available in this build");
        }
        let Some(dp) = dp else {
            if self.strip {
                // ts-blank-space keeps them, types included.
                let erased = self.erased.take();
                while self.is(TokenKind::At) {
                    self.parse_decorator(false)?;
                    self.code().drop();
                }
                self.erased = erased;
                return Ok(());
            }
            return syntax(
                self.tok.start,
                "parameter decorators are only supported with experimentalDecorators",
            );
        };
        let index = self.f.param_count;
        // The thunk is a child of the class's function: the method steps aside while
        // the decorators are parsed (its parameter list has no blocks or iterators).
        let scope = self.scope;
        let method = self.park_function()?;
        let mut t = match dp.thunk.take() {
            Some(t) => t,
            None => self.new_thunk(self.tok.start)?,
        };
        self.resume_function(*t.f);
        let id = intrinsic_id_of!("DecoratorParam");
        while self.is(TokenKind::At) {
            let pos = self.tok.start;
            self.parse_decorator(false)?;
            self.code().push_i32(i32::from(index));
            self.mark(pos);
            self.code().call_intrinsic(id, 2);
            t.count += 1;
        }
        *t.f = self.park_function()?;
        dp.thunk = Some(t);
        self.resume_function(method);
        self.scope = scope;
        Ok(())
    }

    /// Ends `t` and calls it: pushes the decorator array.
    pub(super) fn emit_thunk_call(&mut self, t: Thunk) -> PResult<()> {
        if t.count > u16::MAX as u32 {
            return syntax(t.start, "too many decorators");
        }
        self.resume_function(*t.f);
        self.code().array_from(t.count as u16);
        self.code().return_();
        let child = self.end_function()?;
        self.code().fclosure(child);
        self.code().undefined();
        self.mark(t.start);
        self.code().call(0);
        Ok(())
    }

    /// Drops an unused thunk (the decorators of a TypeScript member without code).
    pub(super) fn discard_thunk(&mut self, t: Thunk) -> PResult<()> {
        self.resume_function(*t.f);
        // Its template is still written: it must be complete.
        self.code().array_from(t.count.min(u16::MAX as u32) as u16);
        self.code().return_();
        self.discard_function()
    }

    /// Adds the decorator `__metadata(key, value)` to `t`, `value` being the type `m`
    /// or with `list` the array of the types `ms`.
    fn thunk_metadata(
        &mut self,
        mut t: Thunk,
        key: &str,
        ms: &[TsMeta],
        list: bool,
    ) -> PResult<Thunk> {
        let pos = t.start;
        self.resume_function(*t.f);
        let a = self.heap.atom_str(key)?;
        self.code().push_atom(a);
        for m in ms {
            self.emit_ts_meta(m, pos)?;
        }
        if list {
            if ms.len() > u16::MAX as usize {
                return syntax(pos, "too many parameters");
            }
            self.code().array_from(ms.len() as u16);
        }
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("DecoratorMetadata"), 2);
        t.count += 1;
        *t.f = self.park_function()?;
        Ok(t)
    }

    /// The `design:type` metadata of a field with the annotation at the current token.
    pub(super) fn thunk_field_metadata(&mut self, t: Thunk) -> PResult<Thunk> {
        let m = self.ts_meta_annotation();
        self.thunk_metadata(t, "design:type", &[m], false)
    }

    /// The metadata of a decorated method (kind `DEC_METHOD`), getter or setter, from
    /// its parameter and return types, as qjs's emit_class_legacy_metadata.
    pub(super) fn thunk_method_metadata(
        &mut self,
        t: Thunk,
        kind: u8,
        dp: &DecParams,
        is_async: bool,
    ) -> PResult<Thunk> {
        let object = TsMeta::new(MetaKind::Object);
        match kind {
            DEC_METHOD => {
                let t = self.thunk_metadata(
                    t,
                    "design:type",
                    &[TsMeta::new(MetaKind::Function)],
                    false,
                )?;
                let t = self.thunk_metadata(t, "design:paramtypes", &dp.types, true)?;
                let ret = match &dp.ret {
                    Some(m) => m.clone(),
                    None if is_async => {
                        let mut m = TsMeta::new(MetaKind::Ref);
                        m.names.push_out(self.heap.atom_str("Promise")?);
                        m
                    }
                    None => TsMeta::new(MetaKind::Void),
                };
                self.thunk_metadata(t, "design:returntype", &[ret], false)
            }
            DEC_GETTER => {
                let ty = dp.ret.clone().unwrap_or(object);
                let t = self.thunk_metadata(t, "design:type", &[ty], false)?;
                self.thunk_metadata(t, "design:paramtypes", &[], true)
            }
            _ => {
                let ty = dp.types.first().cloned().unwrap_or(object);
                let t = self.thunk_metadata(t, "design:type", &[ty], false)?;
                self.thunk_metadata(t, "design:paramtypes", &dp.types, true)
            }
        }
    }

    /// The `design:paramtypes` metadata of the class (its constructor's parameters).
    pub(super) fn thunk_ctor_metadata(&mut self, t: Thunk, types: &[TsMeta]) -> PResult<Thunk> {
        self.thunk_metadata(t, "design:paramtypes", types, true)
    }

    /// With the class on the stack (it stays): applies the legacy decorators of `e`
    /// (tsc's `__decorate(decs, C.prototype, key, desc)`).
    pub(super) fn emit_legacy_apply(&mut self, e: LegacyElem) -> PResult<()> {
        let pos = e.thunk.start;
        self.code().dup();
        if !e.is_static {
            let a = self.heap.atom_str("prototype")?;
            self.mark(pos);
            self.code().get_field(a, 0);
        }
        match e.key {
            DecKey::Atom(a) | DecKey::Private(a) => self.code().push_atom(a),
            DecKey::Computed(v) => {
                let r = self.name_ref(v, pos);
                self.code().name_get(r);
            }
        }
        self.emit_thunk_call(e.thunk)?;
        self.code().push_i8(e.kind as i8);
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("ApplyLegacyDecorators"), 4);
        self.code().drop();
        Ok(())
    }

    /// With the class on the stack: applies the legacy class decorators in `t`,
    /// leaving the new class.
    pub(super) fn emit_legacy_apply_class(&mut self, t: Thunk) -> PResult<()> {
        let pos = t.start;
        self.code().undefined();
        self.emit_thunk_call(t)?;
        self.code().push_i8(DEC_CLASS as i8);
        self.mark(pos);
        self.code()
            .call_intrinsic(intrinsic_id_of!("ApplyLegacyDecorators"), 4);
        Ok(())
    }
}

pub(super) const SLOT_STATIC_EXTRAS: u8 = STATE_STATIC_EXTRAS as u8;
pub(super) const SLOT_CLASS_EXTRAS: u8 = STATE_CLASS_EXTRAS as u8;
