//! The instruction table: every opcode, declared once (docs/compiler.md 4.2).
//!
//! From the single `ops!` invocation below the macro generates:
//!
//! - [`Op`], a `repr(u8)` enum in table order (the interpreter matches on it),
//! - [`OPS`], one [`OpInfo`] per opcode: name, immediates, stack effect, cost, flags,
//! - a typed emitter method per opcode on [`CodeBuf`](super::CodeBuf)
//!   (`buf.get_loc(3)`, `buf.jump(label)`, ...),
//! - [`OPS_HASH`], a hash of the whole table for the snapshot build hash.
//!
//! The decoder, the disassembler and the stack checker work from [`OPS`], so they need no
//! per-opcode code.
//!
//! # Table syntax
//!
//! ```text
//! Name method(imm: Kind, ...) pop N [+ imm K] push M [+ imm K] cost Class [flags A | B];
//! ```
//!
//! `pop N + imm K` means "N plus the value of immediate K" (calls pop their arguments);
//! `push M + imm K` likewise.
//! Stack notation in the docs: `a b -> c` pops `b` then `a` and pushes `c`.

use super::imm::ImmKind;

/// Opcode flags (bits of [`OpInfo::flags`]).
pub mod flags {
    /// May throw a JS exception.
    pub const THROWS: u16 = 1 << 0;
    /// May run user code: pushes a frame (a call, or a slow path through a
    /// continuation, docs/compiler.md 7.3).
    pub const CALLS: u16 = 1 << 1;
    /// Only in phase-1 code; the resolution pass rewrites or removes it. The interpreter
    /// treats it as corrupt bytecode.
    pub const PHASE1: u16 = 1 << 2;
    /// Unconditional jump to its `Target` immediate: no fallthrough.
    pub const JUMP: u16 = 1 << 3;
    /// Conditional jump to its `Target` immediate, or fall through.
    pub const BRANCH: u16 = 1 << 4;
    /// No fallthrough and no static successor (`return`, `throw`).
    pub const TERM: u16 = 1 << 5;
    /// Allocates (may fail with out of memory).
    pub const ALLOC: u16 = 1 << 7;
}

/// Kinds for `special_object` (its `U8` immediate).
pub mod special {
    pub const ARGUMENTS: u8 = 0;
    pub const NEW_TARGET: u8 = 1;
    pub const HOME_OBJECT: u8 = 2;
    pub const ACTIVE_FUNCTION: u8 = 3;
    /// The frame's `this` slot (no allocation).
    pub const THIS: u8 = 4;
}

/// The declaration list of `global_decls idx` (Script GlobalDeclarationInstantiation):
/// `CONSTS[idx..]` holds int32 Values `n_var, n_func, n_let, n_const`, then that many
/// atom ids (as int32) in the same order.
pub mod global_decls {
    use alloc::vec::Vec;

    use crate::heap::Atom;
    use crate::value::Value;

    /// The declared names, by kind.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct GlobalDecls {
        pub vars: Vec<Atom>,
        pub funcs: Vec<Atom>,
        pub lets: Vec<Atom>,
        pub consts: Vec<Atom>,
    }

    impl GlobalDecls {
        pub fn is_empty(&self) -> bool {
            self.vars.is_empty()
                && self.funcs.is_empty()
                && self.lets.is_empty()
                && self.consts.is_empty()
        }

        /// The constant pool entries.
        pub fn encode(&self) -> Vec<Value> {
            let lists = [&self.vars, &self.funcs, &self.lets, &self.consts];
            let mut out: Vec<Value> = lists.iter().map(|l| Value::int(l.len() as i32)).collect();
            for l in lists {
                out.extend(l.iter().map(|a| Value::int(a.id() as i32)));
            }
            out
        }

        /// Reads the list starting at `consts[idx]`; `None` if malformed.
        pub fn decode(consts: &[Value], idx: usize) -> Option<GlobalDecls> {
            let count = |i: usize| -> Option<usize> {
                usize::try_from(consts.get(idx + i)?.as_int()?).ok()
            };
            let counts = [count(0)?, count(1)?, count(2)?, count(3)?];
            let mut at = idx + 4;
            let mut lists: [Vec<Atom>; 4] = Default::default();
            for (list, &n) in lists.iter_mut().zip(&counts) {
                for _ in 0..n {
                    let id = consts.get(at)?.as_int()?;
                    list.push(Atom::from_id(id as u32));
                    at += 1;
                }
            }
            let [vars, funcs, lets, consts] = lists;
            Some(GlobalDecls {
                vars,
                funcs,
                lets,
                consts,
            })
        }
    }
}

/// Capture descriptor words (docs/compiler.md 5.4; after the code in `FuncTemplate::DATA`):
/// where `fclosure` takes each capture's Box from.
pub mod capture_desc {
    /// Set: from the creating closure's captures; clear: from a local slot of the
    /// creating frame.
    pub const OUTER: u32 = 1 << 31;
    /// A lazy template's (D2990): the captured binding is immutable (`const`, a
    /// function expression's own name).
    pub const IMMUTABLE: u32 = 1 << 16;
    /// A lazy template's: reads of the captured binding are TDZ-checked.
    pub const CHECKED: u32 = 1 << 17;

    pub const fn local(slot: u16) -> u32 {
        slot as u32
    }

    pub const fn outer(capture: u16) -> u32 {
        OUTER | capture as u32
    }

    /// `(is_outer, index)`
    pub const fn decode(word: u32) -> (bool, u16) {
        (word & OUTER != 0, word as u16)
    }
}

/// Fuel cost class. The interpreter charges [`CostClass::fuel`] per instruction; leaf
/// operations add charges proportional to their work (docs/compiler.md 7.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u8)]
pub enum CostClass {
    /// Never executed (phase-1 pseudo-ops) or no work (`nop`).
    Free,
    /// Stack shuffles, locals, constants, arithmetic fast paths.
    Basic,
    /// Property, global and iterator access.
    Access,
    /// Allocates a cell.
    Alloc,
    /// Calls and constructs.
    Call,
}

impl CostClass {
    /// Initial weights; tuned later by benchmarks.
    pub const fn fuel(self) -> u32 {
        match self {
            CostClass::Free => 0,
            CostClass::Basic => 1,
            CostClass::Access => 2,
            CostClass::Alloc => 3,
            CostClass::Call => 5,
        }
    }
}

/// Everything known about an opcode, from the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpInfo {
    /// Immediate kinds, in encoding order.
    pub imms: &'static [ImmKind],
    /// Encoded size in bytes: the opcode byte plus the immediates.
    pub size: u8,
    /// Values popped (plus the value of immediate `pop_imm`, if set).
    pub pop: u8,
    pub pop_imm: Option<u8>,
    /// Also pops the declared parameters of the intrinsic in immediate `pop_arity`
    /// (`call_intrinsic8`, whose argument count is the intrinsic's arity).
    pub pop_arity: Option<u8>,
    /// Values pushed (plus the value of immediate `push_imm`, if set).
    pub push: u8,
    pub push_imm: Option<u8>,
    pub cost: CostClass,
    /// [`flags`] bits.
    pub flags: u16,
}

impl OpInfo {
    pub const fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    /// Instructions that never continue with the next one.
    pub const fn no_fallthrough(&self) -> bool {
        self.flags & (flags::JUMP | flags::TERM) != 0
    }
}

macro_rules! ops {
    ($(
        $(#[$doc:meta])*
        $name:ident $method:ident ( $( $iname:ident : $ikind:ident ),* )
            pop $pop:literal $( + imm $pidx:literal )? $( arity $aidx:literal )?
            push $push:literal $( + imm $qidx:literal )?
            cost $cost:ident
            $( flags $( $flag:ident )|+ )? ;
    )*) => {
        /// An opcode. The discriminant is the encoded byte; the order is the table order.
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        #[repr(u8)]
        pub enum Op {
            $( $(#[$doc])* $name, )*
        }

        /// The number of opcodes.
        pub const OP_COUNT: usize = [$( Op::$name ),*].len();

        /// The disassembler names (snake_case, also the emitter method names), indexed by
        /// opcode byte. Apart from [`OPS`]: the run time indexes `OPS`, which puts it in
        /// the module, and the shipped module needs no names (0225, D3092).
        pub const OP_NAMES: [&str; OP_COUNT] = [ $( trim_underscore(stringify!($method)) ),* ];

        /// The instruction table, indexed by opcode byte.
        pub const OPS: &[OpInfo] = &[ $(
            OpInfo {
                imms: &[ $( ImmKind::$ikind ),* ],
                size: 1 $( + ImmKind::$ikind.width() )*,
                pop: $pop,
                pop_imm: opt_u8!($( $pidx )?),
                pop_arity: opt_u8!($( $aidx )?),
                push: $push,
                push_imm: opt_u8!($( $qidx )?),
                cost: CostClass::$cost,
                flags: 0 $( $( | flags::$flag )+ )?,
            },
        )* ];

        impl Op {
            /// All opcodes in table order.
            pub const ALL: [Op; OP_COUNT] = [ $( Op::$name ),* ];
        }

        impl<M: super::code::LabelMode> super::CodeBuf<M> {
            $(
                $(#[$doc])*
                // Emitter names mirror the opcode names (`drop`, `add`, `not`, ...).
                #[allow(clippy::should_implement_trait, unused_mut, unused_assignments)]
                #[inline]
                pub fn $method(&mut self $( , $iname: <super::imm::$ikind as super::imm::Imm>::Ty )* ) {
                    // One out-of-line call per call site (0092).
                    let mut raw = [0u32; 2];
                    let mut _i = 0;
                    $(
                        raw[_i] = <super::imm::$ikind as super::imm::Imm>::raw($iname);
                        _i += 1;
                    )*
                    self.encode(Op::$name, raw[0], raw[1]);
                }
            )*
        }
    };
}

/// `in_` -> `in`: emitter methods that would be Rust keywords end in `_`, the
/// disassembler names do not.
const fn trim_underscore(s: &'static str) -> &'static str {
    let b = s.as_bytes();
    if b.len() > 1 && b[b.len() - 1] == b'_' {
        match core::str::from_utf8(b.split_at(b.len() - 1).0) {
            Ok(t) => t,
            Err(_) => s,
        }
    } else {
        s
    }
}

macro_rules! opt_u8 {
    () => {
        None
    };
    ($v:literal) => {
        Some($v)
    };
}

impl Op {
    /// The opcode for an encoded byte.
    #[inline]
    pub const fn from_u8(b: u8) -> Option<Op> {
        if (b as usize) < OP_COUNT {
            Some(Op::ALL[b as usize])
        } else {
            None
        }
    }

    #[inline]
    pub const fn info(self) -> &'static OpInfo {
        &OPS[self as usize]
    }

    /// Encoded size in bytes (a compile-time constant for a known opcode).
    #[inline]
    pub const fn size(self) -> usize {
        OPS[self as usize].size as usize
    }

    pub const fn name(self) -> &'static str {
        OP_NAMES[self as usize]
    }
}

ops! {
    // ---- Constants and literals ------------------------------------------------------

    /// `-> undefined`
    Undefined undefined() pop 0 push 1 cost Basic;
    /// `-> null`
    Null null() pop 0 push 1 cost Basic;
    /// `-> true`
    PushTrue push_true() pop 0 push 1 cost Basic;
    /// `-> false`
    PushFalse push_false() pop 0 push 1 cost Basic;
    /// `-> n`: a small integer constant.
    PushI8 push_i8(v: I8) pop 0 push 1 cost Basic;
    /// `-> n`: an integer constant.
    PushI32 push_i32(v: I32) pop 0 push 1 cost Basic;
    /// `-> v`: a constant pool entry (non-int32 numbers, strings).
    PushConst push_const(idx: Const) pop 0 push 1 cost Basic;
    /// `-> s`: the string of an atom.
    PushAtom push_atom(atom: Atom) pop 0 push 1 cost Basic;
    /// `-> {}`
    Object object() pop 0 push 1 cost Alloc flags ALLOC;
    /// `v1 .. vn -> array`: an array literal without holes or spread.
    ArrayFrom array_from(n: Count) pop 0 + imm 0 push 1 cost Alloc flags ALLOC;
    /// `array v -> array`
    ArrayPush array_push() pop 2 push 1 cost Basic flags ALLOC;
    /// `array -> array`: append a hole.
    ArrayHole array_hole() pop 1 push 1 cost Basic flags ALLOC;
    /// `array iterable -> array`: append the elements of an iterable (spread).
    ArraySpread array_spread() pop 2 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `-> function`: create a closure of child template `child`.
    FClosure fclosure(child: Child) pop 0 push 1 cost Alloc flags ALLOC;
    /// `pattern flags -> regexp`
    RegExp regexp() pop 2 push 1 cost Alloc flags THROWS | ALLOC;
    /// `-> template object`: tagged templates. `CONSTS[idx]` is the site's object once
    /// created; before that, the index of `n`, `n` cooked strings (or `undefined`) and
    /// `n` raw strings, from which the first run creates and caches it.
    GetTemplateObject get_template_object(idx: Const) pop 0 push 1 cost Alloc flags ALLOC;
    /// `-> v`: `arguments`, `new.target`, home object, ... (function entry only).
    SpecialObject special_object(kind: U8) pop 0 push 1 cost Alloc flags ALLOC;
    /// `-> array`: the actual arguments from `index` on (a rest parameter).
    Rest rest(index: Arg) pop 0 push 1 cost Alloc flags ALLOC;

    // ---- Stack shuffles ----------------------------------------------------------------

    /// `a ->`
    Drop drop() pop 1 push 0 cost Basic;
    /// `a -> a a`
    Dup dup() pop 1 push 2 cost Basic;
    /// `a b -> a b a b`
    Dup2 dup2() pop 2 push 4 cost Basic;
    /// `a b -> b a`
    Swap swap() pop 2 push 2 cost Basic;
    /// `a b -> b`
    Nip nip() pop 2 push 1 cost Basic;
    /// `a b -> b a b`
    Insert2 insert2() pop 2 push 3 cost Basic;
    /// `a b c -> c a b c`
    Insert3 insert3() pop 3 push 4 cost Basic;
    /// `a b c d -> d a b c d`
    Insert4 insert4() pop 4 push 5 cost Basic;
    /// `x a b -> a b x`
    Rot3L rot3l() pop 3 push 3 cost Basic;
    /// `a b x -> x a b`
    Rot3R rot3r() pop 3 push 3 cost Basic;
    /// `x a1 .. an -> x a1 .. an x`: copy the value under the top `n` (`pick 0` is
    /// `dup`). Destructuring reaches its iterator under assignment-target references.
    Pick pick(n: U8) pop 1 + imm 0 push 2 + imm 0 cost Basic;
    /// `x a1 .. an v -> v a1 .. an`: overwrite the value under the top `n` + 1.
    Poke poke(n: U8) pop 2 + imm 0 push 1 + imm 0 cost Basic;

    // ---- Phase 1: scope-generic names and markers (docs/compiler.md 5.2) -------------

    /// `-> v`: read a name.
    NameGet name_get(r: NameRef) pop 0 push 1 cost Free flags PHASE1 | THROWS | CALLS;
    /// `v ->`: assign a name.
    NamePut name_put(r: NameRef) pop 1 push 0 cost Free flags PHASE1 | THROWS | CALLS;
    /// `v -> v`: assign a name, keep the value.
    NameSet name_set(r: NameRef) pop 1 push 1 cost Free flags PHASE1 | THROWS | CALLS;
    /// `v ->`: initialize a declaration (no TDZ check, const allowed).
    NameInit name_init(r: NameRef) pop 1 push 0 cost Free flags PHASE1 | THROWS;
    /// `-> v`: read a name for `typeof` (undeclared globals give `undefined`).
    NameTypeof name_typeof(r: NameRef) pop 0 push 1 cost Free flags PHASE1 | THROWS | CALLS;
    /// Scope entry: TDZ setup, Boxes, hoisted block functions.
    EnterScope enter_scope(scope: Scope) pop 0 push 0 cost Free flags PHASE1;
    /// Scope exit.
    LeaveScope leave_scope(scope: Scope) pop 0 push 0 cost Free flags PHASE1;
    /// Start of a `for (let ...)` iteration: fresh copies of the scope's captured
    /// bindings (`rebox`, CreatePerIterationEnvironment).
    IterScope iter_scope(scope: Scope) pop 0 push 0 cost Free flags PHASE1;
    /// A label position (symbolic labels, docs/compiler.md 4.3).
    Label label(id: LabelId) pop 0 push 0 cost Free flags PHASE1;

    // ---- Locals, arguments, Boxes, captures ----------------------------------------

    /// `-> v`
    GetLoc get_loc(loc: Loc) pop 0 push 1 cost Basic;
    /// `v ->`
    PutLoc put_loc(loc: Loc) pop 1 push 0 cost Basic;
    /// `v -> v`
    SetLoc set_loc(loc: Loc) pop 1 push 1 cost Basic;
    /// `-> v`, ReferenceError if uninitialized (TDZ).
    GetLocChk get_loc_chk(loc: Loc) pop 0 push 1 cost Basic flags THROWS;
    /// `v ->`, ReferenceError if uninitialized.
    PutLocChk put_loc_chk(loc: Loc) pop 1 push 0 cost Basic flags THROWS;
    /// `v -> v`, ReferenceError if uninitialized.
    SetLocChk set_loc_chk(loc: Loc) pop 1 push 1 cost Basic flags THROWS;
    /// Store the uninitialized marker.
    SetLocUninit set_loc_uninit(loc: Loc) pop 0 push 0 cost Basic;
    /// `loc = loc + 1` (`i++;`, `++i;` on an unchecked local, 0086): `inc` in place. An
    /// object takes `%ToNumeric` into the local first, then the instruction runs again.
    IncLoc inc_loc(loc: Loc) pop 0 push 0 cost Basic flags THROWS | CALLS;
    /// `loc = loc - 1`, as `inc_loc`.
    DecLoc dec_loc(loc: Loc) pop 0 push 0 cost Basic flags THROWS | CALLS;
    /// `-> v`
    GetArg get_arg(arg: Arg) pop 0 push 1 cost Basic;
    /// `v ->`
    PutArg put_arg(arg: Arg) pop 1 push 0 cost Basic;
    /// `v -> v`
    SetArg set_arg(arg: Arg) pop 1 push 1 cost Basic;
    /// `-> v`: the value of the Box in local `loc`.
    GetBox get_box(loc: Loc) pop 0 push 1 cost Basic;
    /// `v ->`
    PutBox put_box(loc: Loc) pop 1 push 0 cost Basic;
    /// `v -> v`
    SetBox set_box(loc: Loc) pop 1 push 1 cost Basic;
    /// `-> v` with a TDZ check.
    GetBoxChk get_box_chk(loc: Loc) pop 0 push 1 cost Basic flags THROWS;
    /// `v ->` with a TDZ check.
    PutBoxChk put_box_chk(loc: Loc) pop 1 push 0 cost Basic flags THROWS;
    /// `v -> v` with a TDZ check.
    SetBoxChk set_box_chk(loc: Loc) pop 1 push 1 cost Basic flags THROWS;
    /// `loc = new Box(uninitialized)`
    NewBox new_box(loc: Loc) pop 0 push 0 cost Alloc flags ALLOC;
    /// `v ->`: `loc = new Box(v)`.
    NewBoxInit new_box_init(loc: Loc) pop 1 push 0 cost Alloc flags ALLOC;
    /// `loc = new Box(current value)`: per-iteration bindings.
    Rebox rebox(loc: Loc) pop 0 push 0 cost Alloc flags ALLOC;
    /// `-> v`: the value of the closure's capture `cap`.
    GetCap get_cap(cap: Cap) pop 0 push 1 cost Basic;
    /// `v ->`
    PutCap put_cap(cap: Cap) pop 1 push 0 cost Basic;
    /// `v -> v`
    SetCap set_cap(cap: Cap) pop 1 push 1 cost Basic;
    /// `-> v` with a TDZ check.
    GetCapChk get_cap_chk(cap: Cap) pop 0 push 1 cost Basic flags THROWS;
    /// `v ->` with a TDZ check.
    PutCapChk put_cap_chk(cap: Cap) pop 1 push 0 cost Basic flags THROWS;
    /// `v -> v` with a TDZ check.
    SetCapChk set_cap_chk(cap: Cap) pop 1 push 1 cost Basic flags THROWS;
    /// `-> f`: entry `idx` of the realm's prelude function table (privileged code: a
    /// prelude function another one refers to, D2440).
    GetBuiltin get_builtin(idx: Builtin) pop 0 push 1 cost Basic;
    /// `f ->`: stores entry `idx` of the prelude function table, which has `len` entries
    /// (allocated by the first store).
    PutBuiltin put_builtin(idx: Builtin, len: Count) pop 1 push 0 cost Alloc flags ALLOC;

    // ---- Globals ---------------------------------------------------------------------

    /// `-> v`: global lexical record, then the global object; ReferenceError if missing.
    GetGlobal get_global(atom: Atom, ic: Cache) pop 0 push 1 cost Access flags THROWS | CALLS;
    /// `-> v`: like `get_global`, but `undefined` if missing (`typeof`).
    GetGlobalTypeof get_global_typeof(atom: Atom, ic: Cache) pop 0 push 1 cost Access flags THROWS | CALLS;
    /// `v ->`: ReferenceError if missing, TypeError for a global const.
    PutGlobal put_global(atom: Atom, ic: Cache) pop 1 push 0 cost Access flags THROWS | CALLS;
    /// `v ->`: initialize a Script top-level let/const/class.
    InitGlobalLex init_global_lex(atom: Atom) pop 1 push 0 cost Access flags THROWS | ALLOC;
    /// Script GlobalDeclarationInstantiation from the declaration list in constant `idx`.
    GlobalDecls global_decls(idx: Const) pop 0 push 0 cost Access flags THROWS | ALLOC;
    /// TypeError `'x' is read-only`.
    ThrowConstAssign throw_const_assign(atom: Atom) pop 0 push 0 cost Basic flags THROWS | TERM;

    // ---- Properties --------------------------------------------------------------------

    /// `obj -> v`
    GetField get_field(atom: Atom, ic: Cache) pop 1 push 1 cost Access flags THROWS | CALLS;
    /// `obj -> f obj`: callee and `this` for a method call.
    GetMethod get_method(atom: Atom, ic: Cache) pop 1 push 2 cost Access flags THROWS | CALLS;
    /// `obj v ->`
    PutField put_field(atom: Atom, ic: Cache) pop 2 push 0 cost Access flags THROWS | CALLS | ALLOC;
    /// `obj key -> v`
    GetElem get_elem() pop 2 push 1 cost Access flags THROWS | CALLS;
    /// `obj key -> obj key' v`: `get_elem` keeping the reference for a compound
    /// assignment or update, with `key'` = ToPrimitive(key) (one conversion for the get and
    /// the put, after the RequireObjectCoercible of `obj`).
    GetElem2 get_elem2() pop 2 push 3 cost Access flags THROWS | CALLS;
    /// `key -> key'`: ToPrimitive(key, string) now, for a computed key whose definition
    /// comes after other code (`{ [k]: v }`: ToPropertyKey before `v`). Primitives stay.
    ToPropKey to_propkey() pop 1 push 1 cost Basic flags THROWS | CALLS;
    /// `obj key -> f obj`
    GetElemMethod get_elem_method() pop 2 push 2 cost Access flags THROWS | CALLS;
    /// `obj key v ->`
    PutElem put_elem() pop 3 push 0 cost Access flags THROWS | CALLS | ALLOC;
    /// `obj v -> obj`: CreateDataProperty (literals).
    DefineField define_field(atom: Atom, ic: Cache) pop 2 push 1 cost Access flags THROWS | ALLOC;
    /// `obj key v -> obj`: computed keys.
    DefineElem define_elem() pop 3 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `obj f -> obj`
    DefineGetter define_getter(atom: Atom) pop 2 push 1 cost Access flags THROWS | ALLOC;
    /// `obj f -> obj`
    DefineSetter define_setter(atom: Atom) pop 2 push 1 cost Access flags THROWS | ALLOC;
    /// `obj key f -> obj`
    DefineGetterElem define_getter_elem() pop 3 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `obj key f -> obj`
    DefineSetterElem define_setter_elem() pop 3 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `obj f -> obj`: also sets the home object.
    DefineMethod define_method(atom: Atom) pop 2 push 1 cost Access flags THROWS | ALLOC;
    /// `obj key f -> obj`
    DefineMethodElem define_method_elem() pop 3 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `obj v -> obj`: a literal `__proto__: v` sets the prototype when `v` is an object
    /// or null (other values are ignored).
    SetProto set_proto() pop 2 push 1 cost Basic flags THROWS;
    /// `key f -> key f`: SetFunctionName(f, ToPropertyKey(key), prefix) for an anonymous
    /// function under a computed key (`prefix`: 0 none, 1 "get", 2 "set").
    SetFunctionName set_function_name(prefix: U8) pop 2 push 2 cost Basic flags THROWS | CALLS;
    /// `obj src -> obj`: object spread / rest (flags select the variant).
    CopyDataProps copy_data_props(mode: U8) pop 2 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `obj key -> bool`
    Delete delete() pop 2 push 1 cost Access flags THROWS | CALLS;
    /// `key obj -> bool`
    In in_() pop 2 push 1 cost Access flags THROWS | CALLS;
    /// `-> sym`: a new private name (a private symbol described `#x`, docs/compiler.md
    /// 7.9). `flags`: `heap::cells::flags::SYM_PRIVATE_METHOD` (a method or accessor)
    /// and `SYM_PRIVATE_READONLY` (no setter).
    PrivateSymbol private_symbol(atom: Atom, flags: U8) pop 0 push 1 cost Alloc flags ALLOC;
    /// `obj sym -> v`: `obj.#x` (own properties only, never a proxy trap; a getter is
    /// called).
    GetPrivateField get_private_field() pop 2 push 1 cost Access flags THROWS | CALLS;
    /// `obj sym v ->`: `obj.#x = v` (a setter is called).
    PutPrivateField put_private_field() pop 3 push 0 cost Access flags THROWS | CALLS;
    /// `obj sym v -> obj`: adds a private field (`kind` 0) or method (1), ignoring
    /// extensibility; TypeError if `obj` has it already.
    DefinePrivateField define_private_field(kind: U8) pop 3 push 1 cost Access flags THROWS | ALLOC;
    /// `obj sym getter setter -> obj`: adds a private accessor (`undefined` for a
    /// missing half).
    DefinePrivateAccessor define_private_accessor() pop 4 push 1 cost Access flags THROWS | ALLOC;
    /// `sym obj -> bool`: `#x in obj`.
    PrivateIn private_in() pop 2 push 1 cost Access flags THROWS;
    /// `o -> o.[[GetPrototypeOf]]()`: the base of a super property (from the home
    /// object), or the super constructor (from the active function).
    GetSuper get_super() pop 1 push 1 cost Access flags THROWS;
    /// `this base -> v`: `super.x`, base.[[Get]](x, this).
    GetSuperField get_super_field(atom: Atom) pop 2 push 1 cost Access flags THROWS | CALLS;
    /// `this base v ->`: `super.x = v`, base.[[Set]](x, v, this).
    PutSuperField put_super_field(atom: Atom) pop 3 push 0 cost Access flags THROWS | CALLS | ALLOC;
    /// `this base key -> v`
    GetSuperElem get_super_elem() pop 3 push 1 cost Access flags THROWS | CALLS;
    /// `this base key v ->`
    PutSuperElem put_super_elem() pop 4 push 0 cost Access flags THROWS | CALLS | ALLOC;
    /// `home f -> home f`: sets the home object of `f` (class field initializers).
    SetHomeObject set_home_object() pop 2 push 2 cost Basic;
    /// `parent protoParent ctor -> ctor proto`: ClassDefinitionEvaluation's prototype
    /// and constructor setup. `flags` bit 0: there is a heritage (`parent` and
    /// `protoParent`, checked by %ClassHeritage, are used; otherwise ignored).
    DefineClass define_class(flags: U8) pop 3 push 2 cost Alloc flags THROWS | ALLOC;
    /// `obj f -> obj`: a class method (`kind` 0), getter (1) or setter (2):
    /// non-enumerable, and sets the home object.
    DefineClassMethod define_class_method(atom: Atom, kind: U8) pop 2 push 1 cost Access flags THROWS | ALLOC;
    /// `obj key f -> obj`
    DefineClassMethodElem define_class_method_elem(kind: U8) pop 3 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `obj key v -> obj`: a public class field (CreateDataPropertyOrThrow).
    DefineClassField define_class_field() pop 3 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `v -> v`: TypeError for `null`/`undefined` (destructuring).
    RequireObjectCoercible require_object_coercible() pop 1 push 1 cost Basic flags THROWS;

    // ---- Operators -----------------------------------------------------------------------

    /// `a -> -a`
    Neg neg() pop 1 push 1 cost Basic flags THROWS | CALLS;
    /// `a -> +a` (ToNumber)
    Plus plus() pop 1 push 1 cost Basic flags THROWS | CALLS;
    /// `a -> ~a`
    BNot bnot() pop 1 push 1 cost Basic flags THROWS | CALLS;
    /// `a -> !a`
    Not not() pop 1 push 1 cost Basic;
    /// `a -> typeof a`
    Typeof typeof_() pop 1 push 1 cost Basic;
    /// `a -> a + 1` (numeric)
    Inc inc() pop 1 push 1 cost Basic flags THROWS | CALLS;
    /// `a -> a - 1` (numeric)
    Dec dec() pop 1 push 1 cost Basic flags THROWS | CALLS;
    /// `a -> ToNumeric(a)` (postfix `++`/`--`)
    ToNumeric to_numeric() pop 1 push 1 cost Basic flags THROWS | CALLS;
    /// `a -> ToString(a)` (template substitutions)
    ToString to_string() pop 1 push 1 cost Basic flags THROWS | CALLS | ALLOC;
    /// `a b -> a + b`
    Add add() pop 2 push 1 cost Basic flags THROWS | CALLS | ALLOC;
    /// `a b -> a - b`
    Sub sub() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a * b`
    Mul mul() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a / b`
    Div div() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a % b`
    Mod mod_() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a ** b`
    Pow pow() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a << b`
    Shl shl() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a >> b`
    Sar sar() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a >>> b`
    Shr shr() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a & b`
    BAnd band() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a | b`
    BOr bor() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a ^ b`
    BXor bxor() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a < b`
    Lt lt() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a <= b`
    Le le() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a > b`
    Gt gt() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a >= b`
    Ge ge() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a == b`
    Eq eq() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a != b`
    Ne ne() pop 2 push 1 cost Basic flags THROWS | CALLS;
    /// `a b -> a === b`
    StrictEq strict_eq() pop 2 push 1 cost Basic;
    /// `a b -> a !== b`
    StrictNe strict_ne() pop 2 push 1 cost Basic;
    /// `a b -> a instanceof b`
    Instanceof instanceof() pop 2 push 1 cost Access flags THROWS | CALLS;
    /// `v -> v is null or undefined`
    IsNullish is_nullish() pop 1 push 1 cost Basic;
    /// `v -> v === undefined`
    IsUndefined is_undefined() pop 1 push 1 cost Basic;
    /// `s1 .. sn -> s`: join template literal parts in one allocation.
    Concat concat(n: Count) pop 0 + imm 0 push 1 cost Alloc flags THROWS | ALLOC;

    // ---- Control flow ------------------------------------------------------------------

    /// Unconditional jump.
    Jump jump(target: Target) pop 0 push 0 cost Basic flags JUMP;
    /// `v ->`: jump if ToBoolean(v) is false.
    JumpIfFalse jump_if_false(target: Target) pop 1 push 0 cost Basic flags BRANCH;
    /// `v ->`: jump if ToBoolean(v) is true.
    JumpIfTrue jump_if_true(target: Target) pop 1 push 0 cost Basic flags BRANCH;
    /// `v ->`: throw `v`.
    Throw throw() pop 1 push 0 cost Basic flags THROWS | TERM;
    /// `v ->`: return `v`.
    Return return_() pop 1 push 0 cost Basic flags TERM;
    /// Return `undefined`.
    ReturnUndef return_undef() pop 0 push 0 cost Basic flags TERM;
    /// `f this a1 .. an -> r`
    Call call(argc: Argc) pop 2 + imm 0 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `f a1 .. an -> r`: `new f(...)`, new.target = f.
    Construct construct(argc: Argc) pop 1 + imm 0 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `f this args -> r`: call with an argument array (spread).
    CallSpread call_spread() pop 3 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `f args -> r`
    ConstructSpread construct_spread() pop 2 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `f a1 .. an newTarget -> r`: `super(...)` in a derived constructor: constructs
    /// `f` (the super constructor) with that new.target.
    SuperCall super_call(argc: Argc) pop 2 + imm 0 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `f args newTarget -> r`: `super(...args)` with an argument array (spread).
    SuperCallSpread super_call_spread() pop 3 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `a1 .. an -> r`: privileged code only (docs/compiler.md 7.3).
    CallIntrinsic call_intrinsic(id: Intrinsic, argc: Argc) pop 0 + imm 1 push 1 cost Call flags THROWS | CALLS | ALLOC;
    // ---- Generators and async functions (D1375; docs/compiler.md 7.7) ------------------

    /// Generator function entry: creates the generator object, stores it in local
    /// `generator`, suspends the frame (to resume at the next instruction) and returns the
    /// object to the caller.
    GeneratorStart generator_start(generator: Loc) pop 0 push 0 cost Alloc flags ALLOC;
    /// `v -> received isReturn`: suspends the generator in local `generator`, handing `v` to
    /// the resumer. Resumed by `next(x)`: `x false`; by `return(x)`: `x true` (the code
    /// returns `x`, running finally blocks); `throw(e)` throws `e` here.
    Yield yield_(generator: Loc) pop 1 push 2 cost Alloc flags THROWS | CALLS | ALLOC;
    /// `innerResult -> received isReturn`: `yield` for `yield*` in a sync generator: the
    /// resumer hands `innerResult` (the inner iterator's result object) on as it is,
    /// without reading `value` or wrapping it (GeneratorYield ( innerResult )).
    YieldStar yield_star(generator: Loc) pop 1 push 2 cost Alloc flags THROWS | CALLS | ALLOC;
    /// Async function entry: creates the async state (with the result promise) in local
    /// `state`. The function runs on synchronously.
    AsyncStart async_start(state: Loc) pop 0 push 0 cost Alloc flags ALLOC;
    /// `v -> result`: suspends the async function of local `state` until `v` settles,
    /// then pushes its value or throws its reason here. The first await returns the
    /// result promise to the caller.
    Await await_(state: Loc) pop 1 push 1 cost Alloc flags THROWS | CALLS | ALLOC;
    /// `v ->`: return from an async function: resolves the result promise with `v` and
    /// returns the promise.
    AsyncReturn async_return(state: Loc) pop 1 push 0 cost Call flags THROWS | CALLS | TERM;
    /// `e ->`: an exception leaves an async function: rejects the result promise with
    /// `e` and returns the promise (the target of the body's catch-all handler).
    AsyncReject async_reject(state: Loc) pop 1 push 0 cost Call flags THROWS | CALLS | TERM;
    /// `obj -> iterator`
    GetIterator get_iterator() pop 1 push 1 cost Access flags THROWS | CALLS | ALLOC;
    /// `it next -> r`: `r = next.call(it)`, an iterator result. When `next` is the
    /// original `next` of an Array or String Iterator and that step runs no user code,
    /// `r` is the next value itself and the instruction jumps (no result object, D1800).
    IterNext iter_next(target: Target) pop 2 push 1 cost Call flags THROWS | CALLS | ALLOC | BRANCH;
    /// `-> r`: `iter_next` of the iterator in local `loc` and its `next` in `loc + 1` (a
    /// loop's or pattern's temporaries, D2732): one instruction for for-of's step.
    IterNextLoc iter_next_loc(loc: Loc, target: Target) pop 0 push 1 cost Call flags THROWS | CALLS | ALLOC | BRANCH;
    /// `v -> v`: TypeError if `v` is not an object (iterator results).
    IterCheckObject iter_check_object() pop 1 push 1 cost Basic flags THROWS;
    /// `iterator ->`: call `iterator.return()` if present.
    IteratorClose iterator_close() pop 1 push 0 cost Access flags THROWS | CALLS;
    /// `obj -> for-in iterator` (a snapshot of the enumerable keys)
    ForInStart for_in_start() pop 1 push 1 cost Alloc flags THROWS | ALLOC;
    /// `it -> it key done`
    ForInNext for_in_next() pop 1 push 3 cost Access flags THROWS;
    /// TypeError if a class constructor is called without `new`.
    CheckCtor check_ctor() pop 0 push 0 cost Basic flags THROWS;
    /// `v -> v`: a derived constructor's `this`: ReferenceError while uninitialized
    /// (`undefined`, before `super()`).
    CheckThis check_this() pop 1 push 1 cost Basic flags THROWS;
    /// `v ->`: before `super()` binds `this`: ReferenceError if it is bound already.
    CheckThisUninit check_this_uninit() pop 1 push 0 cost Basic flags THROWS;
    /// `v -> v`: a derived constructor's return value: TypeError unless an object or
    /// `undefined`.
    CheckCtorReturn check_ctor_return() pop 1 push 1 cost Basic flags THROWS;
    /// Throws a new error with message `atom`; `kind` 0: TypeError, 1: ReferenceError.
    ThrowError throw_error(atom: Atom, kind: U8) pop 0 push 0 cost Basic flags THROWS | TERM;
    /// `debugger` (no effect).
    Debugger debugger() pop 0 push 0 cost Free;
    /// No operation.
    Nop nop() pop 0 push 0 cost Free;

    // ---- Short forms (D2443) ----------------------------------------------------------
    //
    // The final-code emitter replaces an instruction by its short form when the operand
    // fits ([`short_form`]); `CodeBuf::finish` shortens jumps. Each behaves exactly as
    // its wide form ([`Op::wide`]).

    /// `get_loc 0`
    GetLoc0 get_loc0() pop 0 push 1 cost Basic;
    /// `get_loc 1`
    GetLoc1 get_loc1() pop 0 push 1 cost Basic;
    /// `get_loc 2`
    GetLoc2 get_loc2() pop 0 push 1 cost Basic;
    /// `get_loc 3`
    GetLoc3 get_loc3() pop 0 push 1 cost Basic;
    /// `put_loc 0`
    PutLoc0 put_loc0() pop 1 push 0 cost Basic;
    /// `put_loc 1`
    PutLoc1 put_loc1() pop 1 push 0 cost Basic;
    /// `put_loc 2`
    PutLoc2 put_loc2() pop 1 push 0 cost Basic;
    /// `put_loc 3`
    PutLoc3 put_loc3() pop 1 push 0 cost Basic;
    /// `get_arg 0`
    GetArg0 get_arg0() pop 0 push 1 cost Basic;
    /// `get_arg 1`
    GetArg1 get_arg1() pop 0 push 1 cost Basic;
    /// `get_arg 2`
    GetArg2 get_arg2() pop 0 push 1 cost Basic;
    /// `get_arg 3`
    GetArg3 get_arg3() pop 0 push 1 cost Basic;
    /// `set_loc 0`
    SetLoc0 set_loc0() pop 1 push 1 cost Basic;
    /// `set_loc 1`
    SetLoc1 set_loc1() pop 1 push 1 cost Basic;
    /// `set_loc 2`
    SetLoc2 set_loc2() pop 1 push 1 cost Basic;
    /// `set_loc 3`
    SetLoc3 set_loc3() pop 1 push 1 cost Basic;
    /// `push_i8 0`
    Push0 push0() pop 0 push 1 cost Basic;
    /// `push_i8 1`
    Push1 push1() pop 0 push 1 cost Basic;
    /// `push_i8 2`
    Push2 push2() pop 0 push 1 cost Basic;
    /// `push_i8 3`
    Push3 push3() pop 0 push 1 cost Basic;
    /// `call 0`
    Call0 call0() pop 2 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `call 1`
    Call1 call1() pop 3 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `call 2`
    Call2 call2() pop 4 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `call 3`
    Call3 call3() pop 5 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `get_loc` of a local below 256.
    GetLoc8 get_loc8(loc: Loc8) pop 0 push 1 cost Basic;
    /// `put_loc` of a local below 256.
    PutLoc8 put_loc8(loc: Loc8) pop 1 push 0 cost Basic;
    /// `set_loc` of a local below 256.
    SetLoc8 set_loc8(loc: Loc8) pop 1 push 1 cost Basic;
    /// `get_arg` of an argument below 256.
    GetArg8 get_arg8(arg: Arg8) pop 0 push 1 cost Basic;
    /// `push_const` of a constant below 256.
    PushConst8 push_const8(idx: Const8) pop 0 push 1 cost Basic;
    /// `a1 .. an -> r`: `call_intrinsic id, n` of an intrinsic below 256 with a fixed
    /// arity `n` (its declared parameters: privileged calls pass exactly those).
    CallIntrinsic8 call_intrinsic8(id: Intrinsic8) pop 0 arity 0 push 1 cost Call flags THROWS | CALLS | ALLOC;
    /// `-> f undefined`: `get_builtin idx; undefined`, a prelude function as the callee
    /// of a call with `this` undefined.
    GetBuiltinCallee8 get_builtin_callee8(idx: Builtin8) pop 0 push 2 cost Basic;
    /// `jump` with an 8-bit offset.
    Jump8 jump8(target: Target8) pop 0 push 0 cost Basic flags JUMP;
    /// `jump_if_false` with an 8-bit offset.
    JumpIfFalse8 jump_if_false8(target: Target8) pop 1 push 0 cost Basic flags BRANCH;
    /// `jump_if_true` with an 8-bit offset.
    JumpIfTrue8 jump_if_true8(target: Target8) pop 1 push 0 cost Basic flags BRANCH;
    /// The whole code of a lazy template (D2990): compiles the function at its first
    /// call and continues in the compiled template, in the same frame.
    Lazy lazy() pop 0 push 0 cost Alloc flags THROWS | ALLOC | TERM;
}

impl Op {
    /// The instruction a short form stands for (itself for the others;
    /// `get_builtin_callee8`, two instructions, too).
    pub const fn wide(self) -> Op {
        match self {
            Op::GetLoc0 | Op::GetLoc1 | Op::GetLoc2 | Op::GetLoc3 | Op::GetLoc8 => Op::GetLoc,
            Op::PutLoc0 | Op::PutLoc1 | Op::PutLoc2 | Op::PutLoc3 | Op::PutLoc8 => Op::PutLoc,
            Op::GetArg0 | Op::GetArg1 | Op::GetArg2 | Op::GetArg3 | Op::GetArg8 => Op::GetArg,
            Op::Call0 | Op::Call1 | Op::Call2 | Op::Call3 => Op::Call,
            Op::SetLoc0 | Op::SetLoc1 | Op::SetLoc2 | Op::SetLoc3 | Op::SetLoc8 => Op::SetLoc,
            Op::Push0 | Op::Push1 | Op::Push2 | Op::Push3 => Op::PushI8,
            Op::PushConst8 => Op::PushConst,
            Op::CallIntrinsic8 => Op::CallIntrinsic,
            Op::Jump8 => Op::Jump,
            Op::JumpIfFalse8 => Op::JumpIfFalse,
            Op::JumpIfTrue8 => Op::JumpIfTrue,
            op => op,
        }
    }

    /// The operand of a form without immediates (`get_loc2`: 2).
    pub const fn implied(self) -> Option<u32> {
        Some(match self {
            Op::GetLoc0 | Op::PutLoc0 | Op::SetLoc0 | Op::GetArg0 | Op::Push0 | Op::Call0 => 0,
            Op::GetLoc1 | Op::PutLoc1 | Op::SetLoc1 | Op::GetArg1 | Op::Push1 | Op::Call1 => 1,
            Op::GetLoc2 | Op::PutLoc2 | Op::SetLoc2 | Op::GetArg2 | Op::Push2 | Op::Call2 => 2,
            Op::GetLoc3 | Op::PutLoc3 | Op::SetLoc3 | Op::GetArg3 | Op::Push3 | Op::Call3 => 3,
            _ => return None,
        })
    }

    /// The short form of a jump (the offset decides whether it fits).
    pub const fn short_jump(self) -> Option<Op> {
        match self {
            Op::Jump => Some(Op::Jump8),
            Op::JumpIfFalse => Some(Op::JumpIfFalse8),
            Op::JumpIfTrue => Some(Op::JumpIfTrue8),
            _ => None,
        }
    }
}

/// The short form of `op` with immediates `a`, `b`, if there is one that encodes them
/// (D2443); jumps excepted, which `CodeBuf::finish` shortens.
pub fn short_form(op: Op, a: u32, b: u32) -> Option<Op> {
    // `[implied 0..4, one byte]`
    let (implied, narrow) = match op {
        Op::GetLoc => (Some(Op::GetLoc0), Some(Op::GetLoc8)),
        Op::PutLoc => (Some(Op::PutLoc0), Some(Op::PutLoc8)),
        Op::GetArg => (Some(Op::GetArg0), Some(Op::GetArg8)),
        Op::Call => (Some(Op::Call0), None),
        Op::SetLoc => (Some(Op::SetLoc0), Some(Op::SetLoc8)),
        Op::PushI8 => (Some(Op::Push0), None),
        Op::PushConst => (None, Some(Op::PushConst8)),
        Op::CallIntrinsic => {
            let fixed = match super::intrinsics::intrinsic(a as u16) {
                Some(i) => !i.variadic && i.params as u32 == b,
                None => false,
            };
            return if a < 256 && fixed {
                Some(Op::CallIntrinsic8)
            } else {
                None
            };
        }
        _ => return None,
    };
    if let Some(base) = implied
        && a < 4
    {
        return Op::from_u8(base as u8 + a as u8);
    }
    if a < 256 { narrow } else { None }
}

const _: () = {
    assert!(OP_COUNT <= 256);
    // Jump targets are always the last immediate: fixups and `Insn::jump_target` rely
    // on the offset being relative to the end of the instruction.
    let mut i = 0;
    while i < OP_COUNT {
        let info = &OPS[i];
        let mut j = 0;
        while j < info.imms.len() {
            if info.imms[j].is_target() {
                assert!(
                    j + 1 == info.imms.len(),
                    "Target must be the last immediate"
                );
                assert!(
                    info.flags & (flags::JUMP | flags::BRANCH) != 0,
                    "only jumps have a Target"
                );
            }
            j += 1;
        }
        let jumpy = info.flags & (flags::JUMP | flags::BRANCH) != 0;
        let n = info.imms.len();
        assert!(
            !jumpy || (n >= 1 && info.imms[n - 1].is_target()),
            "jumps have a Target as their last immediate"
        );
        // Only `iter_next_loc` has another immediate before it (`CodeBuf::finish`
        // shortens only the plain jumps).
        assert!(
            !jumpy || n == 1 || i == Op::IterNextLoc as usize,
            "only iter_next_loc has an immediate before its Target"
        );
        if let Some(k) = info.pop_imm {
            assert!((k as usize) < info.imms.len(), "pop_imm out of range");
        }
        if let Some(k) = info.push_imm {
            assert!((k as usize) < info.imms.len(), "push_imm out of range");
        }
        if let Some(k) = info.pop_arity {
            assert!(
                matches!(info.imms[k as usize], ImmKind::Intrinsic8),
                "pop_arity names an intrinsic"
            );
        }
        // A short form behaves as its wide form: the same stack effect (when the wide
        // form's immediates are implied), cost and flags.
        let op = Op::ALL[i];
        let wide = op.wide().info();
        assert!(wide.cost as u8 == info.cost as u8 && wide.flags == info.flags);
        if let Some(n) = op.implied() {
            let n = n as u8;
            assert!(info.imms.is_empty());
            assert!(info.pop == wide.pop + if wide.pop_imm.is_some() { n } else { 0 });
            assert!(info.push == wide.push);
        }
        i += 1;
    }
};

/// Hash of the instruction table: names, immediates, stack effects, costs, flags and
/// order. Part of the snapshot build hash (with the heap's `LAYOUT_HASH`, see
/// [`build_hash`]): bytecode is only valid for the table it was compiled with.
pub const OPS_HASH: u32 = hash_ops(OPS, &OP_NAMES);

/// FNV-1a over the table.
pub const fn hash_ops(ops: &[OpInfo], names: &[&str]) -> u32 {
    const fn mix(h: u32, b: u8) -> u32 {
        (h ^ b as u32).wrapping_mul(0x0100_0193)
    }
    let mut h: u32 = 0x811c_9dc5;
    let mut i = 0;
    while i < ops.len() {
        let op = &ops[i];
        let name = if i < names.len() {
            names[i].as_bytes()
        } else {
            b""
        };
        let mut j = 0;
        while j < name.len() {
            h = mix(h, name[j]);
            j += 1;
        }
        h = mix(h, 0);
        let mut j = 0;
        while j < op.imms.len() {
            h = mix(h, op.imms[j] as u8 + 1);
            j += 1;
        }
        h = mix(h, 0);
        h = mix(h, op.size);
        h = mix(h, op.pop);
        h = mix(
            h,
            match op.pop_imm {
                Some(k) => k + 1,
                None => 0,
            },
        );
        h = mix(
            h,
            match op.pop_arity {
                Some(k) => k + 1,
                None => 0,
            },
        );
        h = mix(h, op.push);
        h = mix(
            h,
            match op.push_imm {
                Some(k) => k + 1,
                None => 0,
            },
        );
        h = mix(h, op.cost as u8);
        h = mix(h, op.flags as u8);
        h = mix(h, (op.flags >> 8) as u8);
        i += 1;
    }
    h
}

/// Combines the heap layout hash and [`OPS_HASH`] into one build hash for heap images
/// that contain bytecode (snapshots, P4).
pub const fn build_hash(layout_hash: u32, ops_hash: u32) -> u32 {
    (layout_hash ^ ops_hash.rotate_left(16)).wrapping_mul(0x9E37_79B1) ^ ops_hash
}
