//! The layout description: every cell kind, declared once.
//!
//! # Cell header
//!
//! Every cell starts at an 8-aligned offset with an 8-byte header:
//!
//! ```text
//! byte 0     kind      CellKind (0 is invalid)
//! byte 1     gc        GC bits (see `gc_bits`); 0 outside a collection
//! bytes 2-3  flags     u16, meaning depends on the kind (see `flags`)
//! bytes 4-7  size      u32, total cell size in bytes, header included, a multiple of 8
//! ```
//!
//! Cells are laid out back to back, so walking the arena from offset 0 by `size` visits
//! every cell. Offset 0 holds the [`Roots`] cell, which is how offset 0 doubles as null:
//! nothing ever refers to the roots cell.
//!
//! Field offsets below are relative to the cell start and include the header.
//!
//! # Adding a cell kind
//!
//! 1. Declare it in `cells!` below with the next free kind id. Give every field that holds
//!    a reference the type [`Value`] or [`Ref`], never a plain `u32`: those two types *are*
//!    the trace description. The GC (`gc.rs`), the walker and `verify` then handle the kind
//!    automatically, including a tail of values or of a record declared in `records!`.
//! 2. Zero bytes must be a valid initial state for tracing (they are, for `Value` and
//!    `Ref`). Do not cache arena offsets anywhere else: tables keyed by offsets would need
//!    rehashing after every collection.
//! 3. Optionally add kind-specific checks to `walk::cell_invariants`, for anything the heap
//!    code relies on to terminate (counts, ref target kinds).
//!
//! The layout hash changes with any edit here, which invalidates old heap images.

use super::layout::{Field, Ref, cells, records};
use crate::value::Value;

/// The common header fields.
pub struct Header;

impl Header {
    pub const KIND: Field<u8> = Field::new(0);
    pub const GC: Field<u8> = Field::new(1);
    pub const FLAGS: Field<u16> = Field::new(2);
    pub const SIZE: Field<u32> = Field::new(4);
}

/// GC bits in header byte 1. Reserved for the collector; always 0 between collections.
pub mod gc_bits {
    /// Mark bit (the in-place collector, compactor.rs).
    pub const MARK: u8 = 1;
    /// The cell has been copied; the new offset is stored in the first payload word.
    pub const FORWARDED: u8 = 2;
    /// A Map/Set/WeakMap/WeakSet object in the old region whose keys include young cells:
    /// a minor collection rehashes it (set by the mark walk, cleared by the update walk).
    pub const REHASH: u8 = 4;
    /// A slice's parent whose slices a full collection counts (`HASH` holds the sum
    /// meanwhile), then one whose slices get copies (compactor.rs, D2542). Cleared with
    /// the mark.
    pub const COUNTED: u8 = 8;
}

/// Per-kind header flags.
pub mod flags {
    /// Rope: the content has a code unit above 0xFF (some leaf is a [`Str16`](super::Str16)).
    pub const ROPE_TWO_BYTE: u16 = 1;
    /// Rope: flattened. `LEFT` points to the flat string, `RIGHT` is null.
    pub const ROPE_FLATTENED: u16 = 2;
    /// Rope: an append rope (D1610). `LEFT` points to an append buffer (or a frozen one)
    /// and the string is its first `LEN` code units; `RIGHT` is null.
    pub const ROPE_APPEND: u16 = 4;
    /// Str8/Str16: an append buffer (D1610). Spare capacity beyond `LEN` (the used length)
    /// may be written in place. Only append ropes' `LEFT` fields reference it; freezing
    /// clears the flag, after which it is an ordinary flat string.
    pub const STR_APPEND_BUF: u16 = 1;
    /// Slice: a full collection found its parent kept alive by little else than slices;
    /// the slice gets a copy of its units right after the collection (D2542).
    pub const SLICE_DETACH: u16 = 1;
    /// Object: new properties may be added.
    pub const OBJ_EXTENSIBLE: u16 = 1;
    /// Object: the property table holds array-index keys (or the class has exotic
    /// integer-keyed behavior). Element fast paths must then fall back.
    pub const OBJ_INDEXED_PROPS: u16 = 2;
    /// Array: `length` is not writable.
    pub const OBJ_LENGTH_READONLY: u16 = 4;
    /// Function: a builtin method installed by the prelude (`%InstallMethod`). Stack
    /// traces show its frames as `at name (native)`; other prelude frames are hidden.
    pub const OBJ_BUILTIN: u16 = 8;
    /// Function: an `OBJ_BUILTIN` that is a constructor (`%InstallConstructor`). Other
    /// builtins are not constructors (D1310).
    pub const OBJ_BUILTIN_CTOR: u16 = 16;
    /// HostFunction: async (D1374). A call returns a promise and leaves a pending host
    /// call for the embedder instead of parking the VM.
    pub const OBJ_HOST_ASYNC: u16 = 32;
    /// Object: on the prototype chain of a cached prototype property (D1621): adding a
    /// key or changing its prototype bumps `Roots::PROTO_EPOCH`. Never cleared.
    pub const OBJ_IS_PROTO: u16 = 64;
    /// Object: dictionary mode (0083): `PROPS` is a [`PropTable`](super::PropTable) (or
    /// null, an empty dictionary). Without it `PROPS` is a
    /// [`PropValues`](super::PropValues) block (or null, the root shape). One-way.
    pub const OBJ_DICTIONARY: u16 = 128;
    /// Function (a closure): it has an own `length` that is not stored (D2445): the
    /// template's `LENGTH`, `{writable: false, enumerable: false, configurable: true}`,
    /// before every stored key. Deleting it clears the flag; any other change stores it
    /// first (`Heap::materialize_lazy`).
    pub const OBJ_LAZY_LENGTH: u16 = 256;
    /// Function: likewise an own `name`, the string of the template's `NAME` atom (the
    /// empty string for none), after the lazy `length`.
    pub const OBJ_LAZY_NAME: u16 = 512;
    /// BigInt: the value is negative.
    pub const BIGINT_NEG: u16 = 1;
    /// Symbol: a private name (`#x`, D1400). Never an own key of [[OwnPropertyKeys]],
    /// so no reflection, copy or freeze sees it.
    pub const SYM_PRIVATE: u16 = 1;
    /// Symbol: a private method or accessor name (only for the brand check message).
    pub const SYM_PRIVATE_METHOD: u16 = 2;
    /// Symbol: a private method or getter-only accessor: assignment always throws
    /// (before the brand check, like qjs).
    pub const SYM_PRIVATE_READONLY: u16 = 4;
}

/// `"DJS\0"` little-endian, in [`Roots::MAGIC`].
pub const MAGIC: u32 = u32::from_le_bytes(*b"DJS\0");

/// Words per inline cache slot in `FuncTemplate::IC` (D1620; 5 since 0083's addition
/// caches).
pub const IC_WORDS: u32 = 5;
/// Inline cache slot word 0: the slot holds an arena offset (a prototype property slot,
/// D1621); the collector empties such slots.
pub const IC_OFFSET_SLOT: u32 = 1 << 30;

/// `Roots::FLAGS`: the out-of-memory reserve is open (D1600): an out-of-memory error was
/// thrown and the arena may use the bytes held back below the cap.
pub const ROOT_RESERVE_OPEN: u32 = 1;

/// `Roots::FLAGS`: weak references are strong (D1680): new WeakRefs and
/// FinalizationRegistry cells hold their targets in traced tables, so no collection,
/// whenever it runs, is observable.
pub const ROOT_WEAK_STRONG: u32 = 2;

/// `Roots::FLAGS`: frozen builtins (D2304): the objects of the builtin prefix
/// (`Roots::BASE_END`) behave as frozen, and [[Set]] overrides their inherited read-only
/// properties on other receivers.
pub const ROOT_FROZEN_BUILTINS: u32 = 4;

records! {
    /// One property in a [`PropTable`]. For a data property `VALUE` is the value and
    /// `SETTER` is unused (0). For an accessor `VALUE` is the getter and `SETTER` the
    /// setter (`undefined` when absent). A deleted entry has `KEY` 0 and zeroed slots.
    PropEntry[16] {
        /// The key: an [`Atom`](super::Atom) id. 0 marks a deleted entry.
        KEY: u32 = 0,
        /// [`PropFlags`](super::PropFlags) bits, plus `object::STORE_ACCESSOR`.
        FLAGS: u32 = 4,
        /// A data property's value, or an internal ref to the [`AccessorPair`] of an
        /// accessor (D1830).
        VALUE: Value = 8,
    }

    /// One slot of the [`HandleTable`]. A live slot has `NEXT` = [`HANDLE_LIVE`]; a free
    /// slot holds `undefined` and links to the next free slot (`index + 1`, 0 = end).
    HandleSlot[16] {
        VALUE: Value = 0,
        NEXT: u32 = 8,
    }
}

/// [`HandleSlot::NEXT`] of a live handle.
pub const HANDLE_LIVE: u32 = u32::MAX;

cells! {
    /// The root set, always at offset 0. Everything live is reachable from here. The GC
    /// copies this cell first and then scans the to-space from offset 0, so scanning the
    /// roots and scanning the heap are the same loop.
    1 => Roots {
        /// [`MAGIC`].
        MAGIC: u32 = 8,
        /// [`LAYOUT_HASH`] of the build that created the heap.
        VERSION: u32 = 12,
        /// The [`AtomTable`].
        ATOMS: Ref = 16,
        /// The value stack area (interpreter, issue 0006). Null for now.
        STACK: Ref = 20,
        /// The realm record (global object, intrinsics). `undefined` until a realm exists.
        REALM: Value = 24,
        /// The microtask queue (D1370): a [`ValueArray`] of 5-value entries `[f, a0, a1,
        /// a2, a3]` from `Interp::JOB_HEAD` to `LEN`. Null until the first job.
        JOBS: Ref = 32,
        /// The pending host call table.
        HOST_CALLS: Ref = 36,
        /// The [`HandleTable`] (values the embedder or Rust code hold across a GC). Null
        /// until the first handle is created.
        HANDLES: Ref = 40,
        /// The [`Interp`] cell: interpreter registers and run state. Null until a VM
        /// exists.
        INTERP: Ref = 44,
        /// The image's dependency descriptor (D2265, `vm::ImageDeps`), 0 for a bare
        /// heap: the compatibility version (`vm::COMPAT_VERSION`). With `VERSION`,
        /// `FEATURES`, `OPS`, `INTRINSICS` and `RECORDS`, what an image must match to load.
        COMPAT: u32 = 48,
        /// Heap state bits: [`ROOT_RESERVE_OPEN`] (D1600), [`ROOT_WEAK_STRONG`] (D1680),
        /// [`ROOT_FROZEN_BUILTINS`] (D2304).
        FLAGS: u32 = 52,
        /// The arena size at which the next collection is requested (D1601: part of the
        /// image, so a restored VM collects at the same points). 0 for a fresh heap.
        GC_THRESHOLD: u32 = 56,
        /// The prototype epoch of the inline caches (D1621): bumped by key additions and
        /// prototype changes of objects marked [`flags::OBJ_IS_PROTO`].
        PROTO_EPOCH: u32 = 60,
        /// The root [`Shape`] (no properties, 0083), or null until the first shape-mode
        /// property addition.
        ROOT_SHAPE: Ref = 64,
        /// Shapes created so far: the global shape budget (`object::SHAPE_BUDGET`).
        /// Shapes never die (strong transitions, D1831), so this only grows.
        SHAPES: u32 = 68,
        /// The end of the old region (D2041): the cells in `[0, OLD_END)` are live
        /// for minor collections, which neither mark nor move them. A full collection
        /// sets it to the arena size it leaves (everything live becomes old). 0 for a
        /// fresh heap (no old region yet).
        OLD_END: u32 = 72,
        /// Minor collections since the last full one: the schedule of full collections
        /// (`gc::FULL_EVERY`), in the image so that a restored VM collects the same way.
        MINORS: u32 = 76,
        /// The end of the builtin prefix (D022, 0147): the base image's cells, the same
        /// offsets in every VM of a build. Set by `Vm::prepare_base_image` to the image's
        /// length; 0 in a heap without one. Distinct from `OLD_END`, which also covers
        /// what an embedder sealed (pi's prelude).
        BASE_END: u32 = 80,
        /// The descriptor's instruction table hash (`bytecode::OPS_HASH`).
        OPS: u32 = 84,
        /// The descriptor's intrinsic table hash (`bytecode::intrinsics::INTRINSICS_HASH`).
        INTRINSICS: u32 = 88,
        /// The descriptor's hash of the internal id tables (`vm::RECORDS_HASH`, D2266).
        RECORDS: u32 = 92,
        /// The descriptor's feature set (`vm::FEATURES`, D2267).
        FEATURES: u32 = 96,
        /// The key of the VM's keyed string hash (D2433): the hash of the strings, atoms
        /// and Map/Set keys of the VM's own (above `BASE_END`), so that keys crafted to
        /// collide under a known hash don't. Derived from the `Math.random` seed
        /// (`Vm::set_random_seed`); a restored image keeps its own.
        HASH_KEY: u64 = 104,
    };

    /// A flat one-byte (Latin-1) string. Holds only strings with every code unit <= 0xFF;
    /// constructors narrow, so a string that fits here is never a [`Str16`].
    2 => Str8 {
        /// Length in code units (= bytes).
        LEN: u32 = 8,
        /// Cached string hash; 0 means not computed yet.
        HASH: u32 = 12,
    } tail DATA: [u8] = 16;

    /// A flat two-byte (UTF-16LE) string. Always contains a code unit above 0xFF.
    3 => Str16 {
        /// Length in code units.
        LEN: u32 = 8,
        /// Cached string hash; 0 means not computed yet.
        HASH: u32 = 12,
    } tail DATA: [u16] = 16;

    /// A concatenation of two strings, flattened lazily, or an append rope (a prefix of an
    /// append buffer). Header flags: [`flags::ROPE_TWO_BYTE`], [`flags::ROPE_FLATTENED`],
    /// [`flags::ROPE_APPEND`].
    4 => Rope {
        /// Length in code units: `len(LEFT) + len(RIGHT)` (an append rope: at most
        /// `len(LEFT)`).
        LEN: u32 = 8,
        /// Cached string hash; 0 means not computed yet.
        HASH: u32 = 12,
        /// The left string, or the flat result once flattened.
        LEFT: Ref = 16,
        /// The right string; null once flattened.
        RIGHT: Ref = 20,
    };

    /// A slice (D2542): the code units `[OFF, OFF + LEN)` of `PARENT`, a flat one-byte
    /// string (never an append buffer, never another slice). Made instead of a copy for
    /// long substrings of long strings; flattening copies it and turns the cell into a
    /// flattened [`Rope`] in place (same size and field offsets). Header flag:
    /// [`flags::SLICE_DETACH`].
    22 => Slice {
        /// Length in code units.
        LEN: u32 = 8,
        /// Cached string hash; 0 means not computed yet.
        HASH: u32 = 12,
        /// The string the units are read from.
        PARENT: Ref = 16,
        /// The first unit's index in `PARENT`.
        OFF: u32 = 20,
    };

    /// A symbol. Every symbol is also an atom, so it can be a property key.
    5 => Symbol {
        /// The symbol's [`Atom`](super::Atom) id.
        ATOM: u32 = 8,
        /// A string, or `undefined`.
        DESCRIPTION: Value = 16,
    };

    /// An object. Header flags: [`flags::OBJ_EXTENSIBLE`], [`flags::OBJ_INDEXED_PROPS`],
    /// [`flags::OBJ_LENGTH_READONLY`].
    6 => Object {
        /// [`ObjectClass`](super::ObjectClass).
        CLASS: u32 = 8,
        /// The prototype object, or null.
        PROTO: Ref = 12,
        /// The named properties: a [`PropValues`] block (shape mode), a [`PropTable`]
        /// ([`flags::OBJ_DICTIONARY`]), or null when there are none.
        PROPS: Ref = 16,
        /// The dense elements: a [`ValueArray`] (`LEN` = dense length; slots may be
        /// holes), or null.
        ELEMENTS: Ref = 20,
    } tail
        /// Internal slots, class specific. Arrays: slot 0 is the length (a Number).
        SLOTS: [Value] = 24;

    /// Property storage. Small tables are searched linearly; above
    /// `object::LINEAR_MAX` entries a hash index ([`U32Array`]) is built.
    7 => PropTable {
        /// Entries in use, deleted ones included. Entries are in insertion order.
        COUNT: u32 = 8,
        /// Deleted entries (only in hashed mode; linear mode compacts on delete).
        DELETED: u32 = 12,
        /// Hash index: buckets holding `entry + 1`, 0 empty, `u32::MAX` deleted. Null in
        /// linear mode.
        INDEX: Ref = 16,
    } tail ENTRIES: [PropEntry] = 24;

    /// A growable array of values (elements, the atom table, later the value stack).
    /// Slots at and beyond `LEN` are 0.
    8 => ValueArray {
        /// Slots in use.
        LEN: u32 = 8,
    } tail VALUES: [Value] = 16;

    /// A fixed array of `u32` (hash indexes). Not traced.
    9 => U32Array {
        LEN: u32 = 8,
    } tail DATA: [u32] = 12;

    /// The atom table: atom index -> string or symbol, plus the intern hash index. In two
    /// levels in a VM made from a base image (D2431): the base's atoms in `BASE_ENTRIES`
    /// and `BASE_INDEX`, in the builtin prefix and never written, and the VM's own after
    /// them in `ENTRIES` and `INDEX`, above it.
    10 => AtomTable {
        /// Number of string atoms in `ENTRIES` (for `INDEX`'s load factor).
        STRINGS: u32 = 8,
        /// [`ValueArray`] of the atoms from index `len(BASE_ENTRIES)` on (from 0 without
        /// a base; slot 0, the null atom, is unused).
        ENTRIES: Ref = 12,
        /// [`U32Array`] of buckets holding the atom indexes of `ENTRIES`' strings
        /// (0 = empty).
        INDEX: Ref = 16,
        /// The base image's entries, atom indexes `0..len` (slot 0 unused); null
        /// without a base.
        BASE_ENTRIES: Ref = 20,
        /// The intern index of `BASE_ENTRIES`' strings; null without a base.
        BASE_INDEX: Ref = 24,
        /// `len(BASE_ENTRIES)`, the first atom index of `ENTRIES` (0 without a base): read
        /// on every lookup, so it is kept here rather than behind a second reference.
        BASE_COUNT: u32 = 28,
    };

    /// The host handle table: values held by the embedder or by Rust code across a
    /// collection. Grown by copying.
    11 => HandleTable {
        /// Slots ever used (live or free); slots at and beyond are 0.
        LEN: u32 = 8,
        /// Head of the free list: `index + 1`, 0 = empty.
        FREE: u32 = 12,
        /// Live handles.
        LIVE: u32 = 16,
    } tail SLOTS: [HandleSlot] = 24;

    // ---- Compiled code (docs/compiler.md 6.1; encodings in `crate::bytecode`) ----------

    /// An immutable compiled function, one per source function. The meaning of `FLAGS`
    /// belongs to the compiler.
    ///
    /// The code is inline (D2444): `DATA` holds the bytecode (`CODE_LEN` bytes), then the
    /// capture descriptors (`CAPTURES` words), then the handler table (16-byte records),
    /// which fills the rest of the cell (`bytecode::template::handlers`). What a
    /// privileged (prelude) function never has, child templates and debug info, is in a
    /// [`FuncInfo`] cell, so a builtin's template is 48 bytes plus its code.
    12 => FuncTemplate {
        /// Always 0, where a [`ValueArray`] has its `LEN`: a closure's `ELEMENTS` field
        /// is its template (D2448), so the element paths read a closure's elements as an
        /// empty array without checking its class.
        NO_ELEMENTS: u32 = 8,
        /// The function's name, an [`Atom`](super::Atom) id (`name`, stack traces).
        NAME: u32 = 12,
        /// Formal parameter count: the argument area is padded to it.
        PARAMS: u16 = 16,
        /// `fn.length`.
        LENGTH: u16 = 18,
        /// Local slots.
        LOCALS: u16 = 20,
        /// Maximum operand stack depth (from the stack checker).
        STACK: u16 = 22,
        /// Number of captures (Boxes in the closure).
        CAPTURES: u16 = 24,
        /// Inline cache slots of the code (the `Cache` immediates, D1620).
        IC_COUNT: u16 = 26,
        /// Length of the bytecode at the start of `DATA`.
        CODE_LEN: u32 = 28,
        /// [`ValueArray`] of constants (numbers, strings, template-object caches), or null.
        CONSTS: Ref = 32,
        /// [`U32Array`] of `IC_COUNT` inline cache slots (`interp::run::ic`), allocated
        /// the first time one is filled; null before.
        IC: Ref = 36,
        /// [`FuncInfo`]: child templates and debug info, or null for neither.
        INFO: Ref = 40,
        /// Function kind and properties (async, generator, simple params, ...).
        FLAGS: u16 = 44,
        /// A `@native` builtin's index into the generated table (with
        /// `func_flags::NATIVE`), else 0.
        NATIVE: u8 = 46,
        /// A `@fast` builtin's index into `builtins::fast::FAST` (with
        /// `func_flags::FAST`, D1850), else 0.
        FAST: u8 = 47,
    } tail DATA: [u8] = 48;

    /// One compiled source.
    13 => Script {
        /// The file name, a string.
        NAME: Ref = 8,
        /// The source text (D009) as UTF-8 in a [`Bytes`] cell (D2992), or null when
        /// stripped.
        SOURCE: Ref = 12,
        /// The compile options that matter to the code (`CompileOptions::bits`): a dump
        /// compiles the source again to name a suspended frame's slots (D2610).
        OPTIONS: u32 = 16,
        /// An async body's parameter names joined by `,`, a string, or null.
        PARAMS: Ref = 20,
    };

    /// A captured binding, the "Box" of docs/compiler.md 5.4 (named `BoxCell` so it does
    /// not shadow `alloc::boxed::Box`).
    14 => BoxCell {
        VALUE: Value = 8,
    };

    /// Raw bytes (line tables). Not traced.
    15 => Bytes {
        /// Length in bytes.
        LEN: u32 = 8,
    } tail DATA: [u8] = 12;

    /// References to cells that are not JS values (shape transitions).
    16 => RefArray {
        /// Entries in use.
        LEN: u32 = 8,
    } tail REFS: [Ref] = 12;

    /// The parts of a [`FuncTemplate`] that privileged (prelude) functions never have
    /// (D2444): child templates, and the debug info of D1980 (a privileged function's
    /// frames are hidden or `(native)` in traces, its source is `[native code]`, its
    /// locals are never named).
    21 => FuncInfo {
        /// The [`Script`] the function comes from, or null.
        SCRIPT: Ref = 8,
        /// Source span of the function in the script (`Function.prototype.toString`).
        SRC_START: u32 = 12,
        SRC_END: u32 = 16,
        /// [`Bytes`]: the line table, or null.
        LINES: Ref = 20,
        /// [`U32Array`] of atom ids: local names, then capture names (TDZ messages,
        /// disassembler). Null if not recorded.
        NAMES: Ref = 24,
        /// Number of child templates in `CHILDREN`.
        CHILD_COUNT: u32 = 28,
    } tail CHILDREN: [Ref] = 32;

    // ---- Interpreter state (docs/compiler.md 7; ids 17-19 follow 0013's code cells) ----

    /// The interpreter's registers and run state, flushed here whenever `run` returns
    /// (D1086), so a suspended VM is entirely in the arena.
    17 => Interp {
        /// `interp::RunState`.
        STATE: u32 = 8,
        /// Base of the current frame (index into the value stack).
        BP: u32 = 12,
        /// Header of the current frame.
        HP: u32 = 16,
        /// Stack top: slots in use.
        SP: u32 = 20,
        /// Next instruction of the current frame.
        PC: u32 = 24,
        /// Id of the host call the VM is parked on, 0 for none.
        HOST_CALL: u32 = 28,
        /// The next host call id to hand out.
        NEXT_CALL_ID: u32 = 32,
        /// `interp::state` flags of the parked host call (its continuation, whether it
        /// was the entry callee).
        FLAGS: u32 = 36,
        /// The completion value (`Done`) or the exception (`Threw`) of the last call.
        RESULT: Value = 40,
        /// The `interp::RunState` the VM returns to when no frame is active (D1370):
        /// `Idle`, or `Done`/`Threw` once the entry call completed. Jobs run on top.
        OUTCOME: u32 = 48,
        /// Index of the first queued job in the [`Roots::JOBS`] array.
        JOB_HEAD: u32 = 52,
        /// The exception `ThrowPending` throws at the parked call.
        THROW: Value = 56,
        /// [`ValueArray`] of the rejected promises without a handler (D1373), or null.
        REJECTIONS: Ref = 64,
        /// How many of the first [`Interp::REJECTIONS`] were still unhandled at a
        /// microtask checkpoint (D1670): reported, so a later `then` does not withdraw them.
        REPORTED: u32 = 68,
        /// The host inbox (D1670): a [`ValueArray`] of 4-value entries `[promise, ok,
        /// value, now]` of completed async host calls, from `INBOX_HEAD` to `LEN`, applied
        /// one at a time when the job queue is empty. Null until the first result.
        INBOX: Ref = 72,
        /// Index of the first unapplied entry in [`Interp::INBOX`].
        INBOX_HEAD: u32 = 76,
        /// Fuel the last run overshot by (D1740): its last instruction cost more than was
        /// left. The next run that executes instructions starts that much shorter, so a
        /// call's total fuel does not depend on how it was sliced. A Number, 0 for none,
        /// and 0 whenever the VM is quiescent (done, threw, idle).
        FUEL_DEBT: Value = 80,
    };

    /// A realm record: the global object and the intrinsics, indexed by
    /// `interp::Intrinsic`.
    18 => Realm {
        /// The global lexical record (D1220): a hidden ordinary object.
        GLOBAL_LEX: Ref = 8,
        /// [`ValueArray`] indexed by intrinsic id: the JS functions registered by
        /// `%InstallIntrinsic` (D1172), `undefined` for the others.
        INSTALLED: Ref = 12,
    } tail INTRINSICS: [Value] = 16;

    /// A pending host call (docs/compiler.md 7.5).
    19 => HostCall {
        /// The call id handed to the embedder.
        ID: u32 = 8,
        /// The host function id.
        FN: u32 = 12,
        /// [`ValueArray`] of the arguments.
        ARGS: Ref = 16,
        /// The `this` value of the call.
        THIS: Value = 24,
        /// An async host call's promise (D1374); undefined for a sync (parked) call.
        PROMISE: Value = 32,
        /// Bit 0: reported to the embedder by `Vm::new_host_calls`.
        FLAGS: u32 = 40,
    };

    /// The entries of a WeakMap or WeakSet (0038, D1382): key/value pairs like a
    /// [`ValueArray`], but the collector does not trace them. A pair stays alive only if
    /// its key is reachable otherwise (ephemeron semantics), and the pairs of dead keys
    /// are cleared. Slots at and beyond `LEN` are 0.
    20 => EphemeronArray {
        /// Slots in use (two per pair).
        LEN: u32 = 8,
    } tail VALUES: [Value] = 16;

    /// A BigInt primitive (0046, D1460): immutable, the magnitude in little-endian
    /// `u32` limbs without high zero limbs (zero has none). Header flag
    /// [`flags::BIGINT_NEG`]: negative (never set for zero).
    /// The getter and setter of an accessor property (D1830), referenced from its
    /// property entry: entries hold one value, so data properties take 16 bytes.
    /// Never shared between properties.
    25 => AccessorPair {
        GETTER: Value = 8,
        SETTER: Value = 16,
    };

    /// A hidden class (0083, docs/objects.md 7): the keys and attributes of a shape-mode
    /// object's properties, in insertion order. Shapes form a tree from
    /// [`Roots::ROOT_SHAPE`] by property additions; they are shared by every object with
    /// the same key sequence and never die (D1831).
    26 => Shape {
        /// The shape without the last property; null for the root.
        PARENT: Ref = 8,
        /// [`U32Array`] `[used, key0, flags0, key1, flags1, ...]`: the descriptors, shared
        /// along a chain. A shape uses the first `COUNT` pairs; `used` is how many pairs
        /// are written (by the longest shape sharing it). Null for the root.
        DESCS: Ref = 12,
        /// Number of properties (= value slots).
        COUNT: u32 = 16,
        /// [`RefArray`] of the child shapes (`LEN` in use), or null, sorted by the key and
        /// flags of the child's last descriptor.
        TRANSITIONS: Ref = 20,
    };

    /// The property values of a shape-mode object (0083): `VALUES[i]` belongs to
    /// descriptor `i` of `SHAPE`. An accessor's value is an internal ref to its
    /// [`AccessorPair`]. Slots at and beyond `LEN` are 0.
    27 => PropValues {
        /// The object's [`Shape`].
        SHAPE: Ref = 8,
        /// Slots in use: always the shape's `COUNT`.
        LEN: u32 = 12,
    } tail VALUES: [Value] = 16;

    24 => BigInt {
        /// Limb count.
        LEN: u32 = 8,
    } tail LIMBS: [u32] = 12;
}

// The GC stores the forwarding offset in the first payload word, so every cell needs one.
const _: () = {
    let mut i = 0;
    while i < LAYOUTS.len() {
        assert!(
            LAYOUTS[i].min_size >= 16,
            "layout: cells need a payload word"
        );
        i += 1;
    }
};

/// Hash of the layout table and the well-known atom list. Stored in [`Roots::VERSION`];
/// the layout component of an image's dependency descriptor (D2265) and part of the
/// build hash.
pub const LAYOUT_HASH: u32 =
    super::layout::hash_layouts(LAYOUTS, super::atom::WELL_KNOWN_NAMES_FOR_HASH);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::layout::FieldTy;

    #[test]
    fn kinds_match_table() {
        // Kind ids are unique and round-trip; they may have gaps (issues working in
        // parallel take separate ranges).
        let mut seen = alloc::vec::Vec::new();
        for l in LAYOUTS {
            assert!(!seen.contains(&l.kind), "duplicate kind {}", l.kind);
            seen.push(l.kind);
            let k = CellKind::from_u8(l.kind).unwrap();
            assert_eq!(k.layout().name, l.name);
        }
        for b in 0..=255u8 {
            assert_eq!(CellKind::from_u8(b).is_some(), seen.contains(&b));
        }
        assert_eq!(CellKind::from_u8(0), None);
    }

    #[test]
    fn generated_layouts() {
        assert_eq!(Str8::LEN.offset, 8);
        assert_eq!(Str8::DATA.offset, 16);
        assert_eq!(Str8::LAYOUT.min_size, 16);
        assert_eq!(Rope::LAYOUT.min_size, 24);
        assert_eq!(Symbol::LAYOUT.min_size, 24);
        assert_eq!(Object::LAYOUT.min_size, 24);
        assert_eq!(Roots::LAYOUT.min_size, 112);
        assert_eq!(Shape::LAYOUT.min_size, 24);
        assert_eq!(PropValues::VALUES.cell_size(4), Some(48));
        let t = PropTable::LAYOUT.tail.unwrap();
        assert_eq!(t.elem.size, 16);
        assert_eq!(t.elem.name, "PropEntry");
        let traced: alloc::vec::Vec<_> = t
            .elem
            .fields
            .iter()
            .filter(|f| f.ty.is_traced())
            .map(|f| f.name)
            .collect();
        assert_eq!(traced, ["VALUE"]);
        assert_eq!(Rope::LAYOUT.fields[2].ty, FieldTy::Ref);
        assert_eq!(PropTable::ENTRIES.cell_size(2), Some(56));
        assert_eq!(Str16::DATA.cell_size(3), Some(24));
        assert_eq!(Str8::DATA.cell_size(u32::MAX), None);
        assert_eq!(Object::LAYOUT.tail_capacity(40), 2);
        assert_eq!(U32Array::LAYOUT.tail_capacity(16), 1);
    }

    #[test]
    fn layout_hash_is_stable_within_build() {
        assert_ne!(LAYOUT_HASH, 0);
        assert_eq!(
            LAYOUT_HASH,
            super::super::layout::hash_layouts(
                LAYOUTS,
                super::super::atom::WELL_KNOWN_NAMES_FOR_HASH
            )
        );
    }
}
