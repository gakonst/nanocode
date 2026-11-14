//! Per-function compiler state: the scope tree, bindings, name references, captures,
//! constants (docs/compiler.md 2.1, 5.1 to 5.4).
use crate::PushOut;

use alloc::vec::Vec;

use crate::bytecode::{CodeBuf, Phase1};
use crate::heap::Atom;
use crate::value::Value;

pub(super) type ScopeId = u32;
pub(super) type VarId = u32;
pub(super) type FuncId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ScopeKind {
    /// The function scope: parameters, `var`s, body-level declarations (Script: the
    /// global scope of the script).
    Body,
    /// The scope that binds a named function expression's own name, outside the body.
    FuncName,
    Block,
    /// The scope of a `for (let ...)` head.
    For,
    /// A `switch` case block: jumps land in its middle, so TDZ checks are never elided.
    Switch,
    /// A `catch` clause: the parameter and the declarations of the catch block.
    Catch,
    /// The block of a `catch` clause with a destructuring parameter: closures in the
    /// pattern must not see its declarations (D1820). Its lexical names still conflict
    /// with the parameter's.
    CatchBody,
    /// A class: the inner class binding (around the heritage), then a nested one for
    /// the private names and the hidden bindings of the class body (compiler/class.rs).
    Class,
}

pub(super) struct Scope {
    pub parent: Option<ScopeId>,
    /// Source offset where the scope starts (the function's start for the body): how a
    /// suspension map names a binding's declaring scope (D2563).
    #[cfg(feature = "dump")]
    pub pos: u32,
    pub kind: ScopeKind,
    pub vars: Vec<VarId>,
    /// Function declarations hoisted to this scope's entry: `(binding, child index)`.
    pub funcs: Vec<(VarId, u32)>,
    /// `var`s that start with the value of a parameter at this scope's entry, before
    /// the function declarations: `(var, parameter)` (`FuncState::var_scope`).
    pub copies: Vec<(VarId, VarId)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum VarKind {
    Var,
    Let,
    Const,
    /// A function declaration: initialized at its scope's entry (hoisted).
    Func,
    /// A parameter. Simple parameter lists keep them in argument slots; otherwise they
    /// are locals initialized in order, with TDZ (see `Var::init_end`).
    Param,
    /// The `this` pseudo-binding (docs/compiler.md 5.7).
    This,
    /// `arguments` (non-arrow functions).
    Arguments,
    /// `new.target`.
    NewTarget,
    /// A named function expression's own name (immutable).
    FuncName,
    /// A `catch (e)` parameter, initialized by the handler.
    CatchParam,
    /// A compiler-internal local that no source name can reach (the generator or async
    /// state object of `generator_start`/`async_start`, docs/compiler.md 7.7).
    Internal,
    /// The home object of a method (`special_object HOME_OBJECT`), for `super.x`.
    HomeObject,
    /// The running function (`special_object ACTIVE_FUNCTION`): a derived
    /// constructor's, for `super()`.
    ActiveFunc,
    /// A private name `#x` of a class (D1400): its private symbol, created at the class
    /// scope's entry. `PrivateMethod` for methods and getters, `PrivateAccessor` for
    /// accessors with a setter.
    PrivateName,
    PrivateMethod,
    PrivateAccessor,
    /// A hidden class binding (a method closure for a private method, a computed field
    /// key, the field initializer): written once while the class is defined, before
    /// anything can read it, so never TDZ-checked.
    Hidden,
    /// A construct's state while it runs (a loop's iterator record, a switch's
    /// discriminant, a pattern's source, D2732): named `construct@offset` like the
    /// temporaries of suspension maps (D2721), written before anything reads it.
    Temp,
    /// A module's import binding (immutable; its Box is the exporting module's, so reads
    /// are always TDZ-checked), and the hidden bindings the linker initializes (a
    /// namespace import, `import.meta`).
    Import,
}

impl VarKind {
    pub fn is_lexical(self) -> bool {
        matches!(self, VarKind::Let | VarKind::Const) || (crate::MODULES && self == VarKind::Import)
    }

    /// Assignment throws a TypeError.
    pub fn is_immutable(self) -> bool {
        matches!(self, VarKind::Const | VarKind::FuncName)
            || (crate::MODULES && self == VarKind::Import)
    }

    /// Set up by the prologue from `special_object` (docs/compiler.md 5.7); no storage
    /// unless referenced.
    pub fn is_pseudo(self) -> bool {
        matches!(
            self,
            VarKind::This
                | VarKind::Arguments
                | VarKind::NewTarget
                | VarKind::FuncName
                | VarKind::HomeObject
                | VarKind::ActiveFunc
        )
    }
}

impl Var {
    /// A module's top-level binding: its index in the module environment.
    #[inline(always)]
    pub fn env(&self) -> Option<u16> {
        (crate::MODULES && self.kind != VarKind::Param && self.param != 0).then(|| self.param - 1)
    }
}

pub(super) struct Var {
    pub name: Atom,
    pub kind: VarKind,
    /// Where the binding lives (for `var`: the body scope).
    pub scope: ScopeId,
    /// Source offset where initialization completes; accesses at or after it need no TDZ
    /// check (docs/compiler.md 5.6). `u32::MAX` until the declaration is complete; 0 for
    /// bindings that are never in TDZ.
    pub init_end: u32,
    /// Script top-level `var`/`let`/`const`/function: a global binding, not a local.
    pub global: bool,
    /// Parameter index for `Param`; for other bindings 0, or a module's top-level
    /// binding's index in the module environment plus 1 (the module closure's captures,
    /// compiler/module.rs; `Var::env`).
    pub param: u16,
    /// Some inner function refers to it: it lives in a Box.
    pub captured: bool,
    /// A prelude function another function refers to: it lives in the realm's prelude
    /// function table (privileged code, D2440).
    pub builtin: bool,
    /// Referenced at all (for the pseudo-bindings, which cost prologue code).
    pub used: bool,
    /// The TypeScript enum or namespace this binding holds (const enum folding).
    pub ts_decl: Option<u32>,
}

/// A name reference in phase-1 code: the `name_*` op's immediate indexes this table.
#[derive(Clone, Copy, Debug)]
pub(super) struct NameRef {
    pub atom: Atom,
    /// The innermost scope at the reference.
    pub scope: ScopeId,
    /// Source offset of the identifier.
    pub pos: u32,
    /// The reference initializes its binding (`name_init`): never TDZ-checked.
    pub init: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FuncKind {
    Script,
    AsyncBody,
    /// Indirect eval code (`Goal::Eval`): top level like a sync codemode body.
    Eval,
    /// A function declaration or expression.
    Normal,
    Arrow,
    /// Object literal methods and accessors (no `prototype`, not constructors).
    Method,
    Getter,
    Setter,
    /// Class constructors (base and derived).
    ClassCtor,
    DerivedCtor,
    /// A class's instance or static field initializer function (compiler/class.rs).
    FieldInit,
    /// A class static block, called by the static initializer.
    StaticBlock,
    /// A module's body (`Goal::Module`): strict, `this` undefined, its top-level
    /// bindings in the module environment (compiler/module.rs). Last: lazy records store
    /// kinds by number (compiler/lazy.rs `KINDS`).
    Module,
}

impl FuncKind {
    /// Has its own `this`/`arguments`/`new.target` (arrows inherit them).
    pub fn has_own_this(self) -> bool {
        !matches!(self, FuncKind::Arrow)
    }

    /// Has a home object: `super.x` is allowed.
    pub fn is_method(self) -> bool {
        matches!(
            self,
            FuncKind::Method
                | FuncKind::Getter
                | FuncKind::Setter
                | FuncKind::ClassCtor
                | FuncKind::DerivedCtor
                | FuncKind::FieldInit
                | FuncKind::StaticBlock
        )
    }

    /// Class field initializers and static blocks: `arguments` is an error there.
    pub fn is_class_init(self) -> bool {
        matches!(self, FuncKind::FieldInit | FuncKind::StaticBlock)
    }

    pub fn is_top_level(self) -> bool {
        matches!(
            self,
            FuncKind::Script | FuncKind::AsyncBody | FuncKind::Eval
        ) || (crate::MODULES && self == FuncKind::Module)
    }
}

/// Where a closure takes a capture's Box from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Capture {
    /// A binding of the parent function (its storage is a Box in a local slot).
    Local(VarId),
    /// Capture `n` of the parent function.
    Outer(u16),
}

/// A try region `[start, end)` whose exceptions go to `target` with the operand stack
/// cut to `depth` (then the exception is pushed). With `depth_from`, `depth` is
/// relative: it is added to the operand depth at that label, which resolution finds
/// from the final code (expressions do not track their absolute depth).
#[derive(Clone, Copy, Debug)]
pub(super) struct HandlerLabels {
    pub start: crate::bytecode::Label,
    pub end: crate::bytecode::Label,
    pub target: crate::bytecode::Label,
    pub depth: u32,
    pub depth_from: Option<crate::bytecode::Label>,
}

pub(super) struct FuncState {
    pub id: FuncId,
    pub kind: FuncKind,
    /// The enclosing function and the scope in it where this one was created.
    pub parent: Option<(FuncId, ScopeId)>,
    /// Child functions in creation order: `fclosure n` creates `children[n]`.
    pub children: Vec<FuncId>,
    /// The function's name (`name` property, stack traces).
    pub name: Atom,
    pub code: CodeBuf<Phase1>,
    pub scopes: Vec<Scope>,
    pub vars: Vec<Var>,
    pub refs: Vec<NameRef>,
    /// Filled by resolution: what each capture slot holds.
    pub captures: Vec<Capture>,
    pub capture_names: Vec<Atom>,
    pub consts: Vec<Value>,
    /// Try regions in phase-1 labels, innermost first (docs/compiler.md 7.4).
    pub handlers: Vec<HandlerLabels>,
    /// `(phase-1 code position, source offset)`, in increasing code position.
    pub lines: Vec<(u32, u32)>,
    /// The phase-1 position of the last `call` that qjs would turn into a tail call
    /// when a `return` follows it directly (`f(x)`, `o.m(x)`, a tagged template; not
    /// spread, optional or `eval` calls). `parse_return` gives it the statement's
    /// position, as qjs's tail calls report (D2031).
    pub tail_call: Option<u32>,
    /// Try blocks and catch clauses being parsed: a `return` there is no tail call in
    /// qjs (it pops the catch offset first).
    pub open_trys: u32,
    /// `var` declarations as `(name, scope where the declaration appeared)`, for
    /// var/let conflicts.
    pub var_decls: Vec<(Atom, ScopeId)>,
    /// Parameters in order (for `Param` bindings).
    pub params: Vec<VarId>,
    /// Formal parameter count without a rest parameter (the argument area).
    pub param_count: u16,
    /// `fn.length`: parameters before the first default or rest.
    pub length: u16,
    /// Only plain identifiers: parameters stay in their argument slots.
    pub simple_params: bool,
    /// The body's directive prologue has "use strict".
    pub use_strict: bool,
    /// D015: sloppy `this` binding (set by compiler/dynamic.rs).
    pub sloppy_this: bool,
    /// Index of a rest parameter.
    pub rest: Option<u16>,
    pub this_var: Option<VarId>,
    pub arguments_var: Option<VarId>,
    pub new_target_var: Option<VarId>,
    pub func_name_var: Option<VarId>,
    pub home_var: Option<VarId>,
    pub active_var: Option<VarId>,
    /// Class constructors: the hidden binding of their class's instance field
    /// initializer (compiler/class.rs).
    pub fields_atom: Atom,
    /// Derived constructors: the epilogue label and the local its returns store the
    /// value in (`Parser::emit_ctor_epilogue`).
    pub ctor_return: Option<(crate::bytecode::Label, Atom)>,
    /// A `return` jumped to that epilogue (falling off the body does not).
    pub ctor_return_used: bool,
    /// `async` function (or the codemode body): `await` is an operator.
    pub is_async: bool,
    /// Generator function: `yield` is an operator.
    pub is_generator: bool,
    /// The local holding the generator/async state (docs/compiler.md 7.7).
    pub state_var: Option<VarId>,
    /// Parsing the formal parameters: `await`/`yield` expressions are errors there.
    pub in_params: bool,
    /// Async functions: the start of the region whose exceptions reject the promise.
    pub async_region: Option<crate::bytecode::Label>,
    /// Source span (for `Function.prototype.toString`, D009).
    pub src_start: u32,
    pub src_end: u32,
    /// TypeScript parameter properties of a class constructor (`constructor(public x)`).
    pub param_props: Vec<Atom>,
    /// The closure of a TypeScript enum or namespace block: its declaration record
    /// (compiler/ts_decl.rs). Free names that are members resolve to properties of the
    /// closure's parameter.
    pub ns: Option<u32>,
    /// The scope of the body's `var`s and function declarations: `BODY_SCOPE`, or with
    /// a non-simple parameter list a separate scope inside it, so that closures in the
    /// parameters do not see them (FunctionDeclarationInstantiation step 28, D1820).
    pub var_scope: ScopeId,
    /// `(scope, name)` → binding, once the function has many (0092).
    pub index: VarIndex,
    /// The construct temporaries (`VarKind::Temp`, compiler/temps.rs): the binding, the
    /// construct and its source offset, which name its slot (D2721, D2732).
    pub temps: Vec<(VarId, &'static str, u32)>,
    scope_pool: Vec<Scope>,
    /// Recording for a suspension map (D2563, compiler/smap.rs), when requested.
    #[cfg(feature = "dump")]
    pub smap: Option<alloc::boxed::Box<super::smap::Recorder>>,
    /// Lazy compilation (D2990): a function written in source that may compile at its
    /// first call (the rest of the rule is in `resolve::lazy_roots`).
    #[cfg(feature = "lazy")]
    pub lazy_ok: bool,
    /// Where its parameters start (a type parameter list, `(`, or an arrow's head):
    /// where a lazy compile starts parsing.
    #[cfg(feature = "lazy")]
    pub head: u32,
    /// The end of the tokens that call it right where it is defined (`fn(`, `(fn)(`),
    /// or 0.
    #[cfg(feature = "lazy")]
    pub call_end: u32,
    /// An arrow: the kind of the function that owns its `this`; `after_q` of
    /// `parse_arrow`.
    #[cfg(feature = "lazy")]
    pub owner: FuncKind,
    #[cfg(feature = "lazy")]
    pub after_q: bool,
}

/// A hash index of the bindings by `(scope, name)`: the first one declared with that
/// name in that scope, as the linear search finds it (0092: typescript.js's top-level
/// closure has thousands of bindings in one scope, and every reference was a linear
/// scan). Built only once a function has `VarIndex::MIN` bindings.
#[derive(Default)]
pub struct VarIndex {
    /// Open addressing; key 0 is empty, else `(scope + 1) << 32 | atom id`.
    slots: Vec<(u64, VarId)>,
    len: usize,
}

impl VarIndex {
    const MIN: usize = 32;

    fn key(scope: ScopeId, name: Atom) -> u64 {
        (u64::from(scope) + 1) << 32 | u64::from(name.id())
    }

    fn slot(&self, key: u64) -> usize {
        let h = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        (h >> 32) as usize & (self.slots.len() - 1)
    }

    fn get(&self, key: u64) -> Option<VarId> {
        let mut i = self.slot(key);
        loop {
            match self.slots[i] {
                (0, _) => return None,
                (k, v) if k == key => return Some(v),
                _ => i = (i + 1) & (self.slots.len() - 1),
            }
        }
    }

    /// Adds `v` unless the key has a binding already.
    fn clear(&mut self) {
        if self.len != 0 {
            self.slots.fill((0, 0));
            self.len = 0;
        }
    }

    fn insert(&mut self, key: u64, v: VarId) {
        if (self.len + 1) * 2 > self.slots.len() {
            let cap = (self.slots.len() * 2).max(64);
            let old = core::mem::replace(&mut self.slots, alloc::vec![(0, 0); cap]);
            self.len = 0;
            for (k, v) in old {
                if k != 0 {
                    self.insert(k, v);
                }
            }
        }
        let mut i = self.slot(key);
        loop {
            match self.slots[i] {
                (0, _) => {
                    self.slots[i] = (key, v);
                    self.len += 1;
                    return;
                }
                (k, _) if k == key => return,
                _ => i = (i + 1) & (self.slots.len() - 1),
            }
        }
    }
}

pub(super) const BODY_SCOPE: ScopeId = 0;

#[derive(Default)]
pub(super) struct Spare {
    children: Vec<FuncId>,
    code: CodeBuf<Phase1>,
    scopes: Vec<Scope>,
    vars: Vec<Var>,
    refs: Vec<NameRef>,
    captures: Vec<Capture>,
    capture_names: Vec<Atom>,
    consts: Vec<Value>,
    handlers: Vec<HandlerLabels>,
    lines: Vec<(u32, u32)>,
    var_decls: Vec<(Atom, ScopeId)>,
    params: Vec<VarId>,
    param_props: Vec<Atom>,
    index: VarIndex,
    temps: Vec<(VarId, &'static str, u32)>,
    scope_pool: Vec<Scope>,
}

impl FuncState {
    pub fn reusing(
        id: FuncId,
        kind: FuncKind,
        parent: Option<(FuncId, ScopeId)>,
        spare: Spare,
    ) -> Self {
        Self::build(id, kind, parent, spare, true)
    }

    pub fn vacant() -> Self {
        Self::build(0, FuncKind::Script, None, Spare::default(), false)
    }

    fn build(
        id: FuncId,
        kind: FuncKind,
        parent: Option<(FuncId, ScopeId)>,
        spare: Spare,
        open: bool,
    ) -> Self {
        let Spare {
            children,
            code,
            mut scopes,
            vars,
            refs,
            captures,
            capture_names,
            consts,
            handlers,
            lines,
            var_decls,
            params,
            param_props,
            index,
            temps,
            mut scope_pool,
        } = spare;
        if open {
            scopes.push(Self::pooled_scope(
                &mut scope_pool,
                None,
                ScopeKind::Body,
                0,
            ));
        }
        FuncState {
            id,
            kind,
            parent,
            children,
            name: Atom::NULL,
            code: if open {
                CodeBuf::phase1_from(code)
            } else {
                code
            },
            scopes,
            vars,
            refs,
            captures,
            capture_names,
            consts,
            handlers,
            lines,
            var_decls,
            params,
            param_count: 0,
            length: 0,
            simple_params: true,
            use_strict: false,
            sloppy_this: false,
            rest: None,
            this_var: None,
            arguments_var: None,
            new_target_var: None,
            func_name_var: None,
            home_var: None,
            active_var: None,
            fields_atom: Atom::NULL,
            ctor_return: None,
            ctor_return_used: false,
            is_async: false,
            is_generator: false,
            tail_call: None,
            open_trys: 0,
            state_var: None,
            in_params: false,
            async_region: None,
            src_start: 0,
            src_end: 0,
            param_props,
            ns: None,
            var_scope: BODY_SCOPE,
            index,
            temps,
            scope_pool,
            #[cfg(feature = "dump")]
            smap: None,
            #[cfg(feature = "lazy")]
            lazy_ok: false,
            #[cfg(feature = "lazy")]
            head: 0,
            #[cfg(feature = "lazy")]
            call_end: 0,
            #[cfg(feature = "lazy")]
            owner: FuncKind::Normal,
            #[cfg(feature = "lazy")]
            after_q: false,
        }
    }

    pub fn recycle(self) -> Spare {
        let FuncState {
            mut children,
            code,
            mut scopes,
            mut vars,
            mut refs,
            mut captures,
            mut capture_names,
            mut consts,
            mut handlers,
            mut lines,
            mut var_decls,
            mut params,
            mut param_props,
            mut index,
            mut temps,
            mut scope_pool,
            ..
        } = self;
        children.clear();
        scope_pool.extend(scopes.drain(..).map(|mut scope| {
            scope.vars.clear();
            scope.funcs.clear();
            scope.copies.clear();
            scope
        }));
        vars.clear();
        refs.clear();
        captures.clear();
        capture_names.clear();
        consts.clear();
        handlers.clear();
        lines.clear();
        var_decls.clear();
        params.clear();
        param_props.clear();
        index.clear();
        temps.clear();
        Spare {
            children,
            code,
            scopes,
            vars,
            refs,
            captures,
            capture_names,
            consts,
            handlers,
            lines,
            var_decls,
            params,
            param_props,
            index,
            temps,
            scope_pool,
        }
    }

    pub fn new_scope(&mut self, parent: ScopeId, kind: ScopeKind, pos: u32) -> ScopeId {
        let scope = Self::pooled_scope(&mut self.scope_pool, Some(parent), kind, pos);
        self.scopes.push_out(scope);
        (self.scopes.len() - 1) as ScopeId
    }

    fn pooled_scope(
        pool: &mut Vec<Scope>,
        parent: Option<ScopeId>,
        kind: ScopeKind,
        pos: u32,
    ) -> Scope {
        #[cfg(not(feature = "dump"))]
        let _ = pos;
        match pool.pop() {
            Some(mut scope) => {
                scope.parent = parent;
                scope.kind = kind;
                #[cfg(feature = "dump")]
                {
                    scope.pos = pos;
                }
                scope
            }
            None => Scope {
                parent,
                #[cfg(feature = "dump")]
                pos,
                kind,
                vars: Vec::new(),
                funcs: Vec::new(),
                copies: Vec::new(),
            },
        }
    }

    pub fn add_var(&mut self, name: Atom, kind: VarKind, scope: ScopeId) -> VarId {
        let id = self.vars.len() as VarId;
        let immediate = !(matches!(kind, VarKind::Let | VarKind::Const | VarKind::Param)
            || (crate::MODULES && kind == VarKind::Import));
        self.vars.push_out(Var {
            name,
            kind,
            scope,
            init_end: if immediate { 0 } else { u32::MAX },
            global: false,
            param: 0,
            captured: false,
            builtin: false,
            used: false,
            ts_decl: None,
        });
        self.scopes[scope as usize].vars.push_out(id);
        if !self.index.slots.is_empty() {
            self.index.insert(VarIndex::key(scope, name), id);
        } else if self.vars.len() >= VarIndex::MIN {
            for (i, v) in self.vars.iter().enumerate() {
                self.index
                    .insert(VarIndex::key(v.scope, v.name), i as VarId);
            }
        }
        id
    }

    /// The binding named `name` declared directly in `scope`.
    pub fn find_in_scope(&self, scope: ScopeId, name: Atom) -> Option<VarId> {
        if !self.index.slots.is_empty() {
            return self.index.get(VarIndex::key(scope, name));
        }
        self.scopes[scope as usize]
            .vars
            .iter()
            .copied()
            .find(|&v| self.vars[v as usize].name == name)
    }

    /// Resolves `name` from `scope` outward within this function.
    pub fn lookup(&self, mut scope: ScopeId, name: Atom) -> Option<VarId> {
        loop {
            if let Some(v) = self.find_in_scope(scope, name) {
                return Some(v);
            }
            scope = self.scopes[scope as usize].parent?;
        }
    }

    /// `inner` is `outer` or nested in it.
    pub fn scope_within(&self, mut inner: ScopeId, outer: ScopeId) -> bool {
        loop {
            if inner == outer {
                return true;
            }
            match self.scopes[inner as usize].parent {
                Some(p) => inner = p,
                None => return false,
            }
        }
    }

    /// The capture slot for `c`, adding it if new.
    pub fn capture_slot(&mut self, c: Capture, name: Atom) -> u16 {
        if let Some(i) = self.captures.iter().position(|&x| x == c) {
            return i as u16;
        }
        self.captures.push_out(c);
        self.capture_names.push_out(name);
        (self.captures.len() - 1) as u16
    }
}
