//! The builtin registry (D2371, issue 0145): stable ids for the builtin objects, functions
//! and symbols of the realm and for the intrinsics, keyed by a spec-style path.
//!
//! The registry is a committed, append-only table (tests/builtins/registry.tsv): an id is
//! never renumbered or reused, and a builtin that goes away keeps its line as a tombstone.
//! It names builtins across djs builds, where offsets, intrinsic ids and the base image's
//! layout change: snapshot exports (docs/goldilocks-snapshots.md, tier 2) and tools such as
//! the REPL's `.heap` name a builtin by its id or path.
//!
//! **Paths.** A *value* entry is a path that leads from a root to an object or symbol of the
//! base realm, the way the spec writes intrinsics without the `%`s:
//! - roots: `globalThis`; a global's name (`Array`); a realm intrinsic that no global reaches
//!   (`%ArrayIteratorPrototype%`, `%TypedArray%`); a prelude function registered for an
//!   intrinsic (`%intrinsic:PromiseResolve%`); a prelude function in the realm's prelude
//!   function table, by its source name (`%prelude:LengthOfArrayLike%`, D2440): the
//!   internal functions builtins call and no property reaches;
//! - segments: `.key` for identifier keys, `["key"]` for others, `[Symbol.iterator]` for
//!   well-known symbol keys, `.[[Prototype]]`;
//! - accessor functions are prefixed `get ` or `set ` (`get Map.prototype.size`).
//!
//! An *intrinsic* entry names an entry of the intrinsics table (bytecode/intrinsics.rs) by
//! its name: the kinds of builtin closures (`PromiseResolveFunction`-like records hold an
//! intrinsic id) and the operands of `call_intrinsic`.
//!
//! Several registered paths may lead to the same object (a new alias); the object's id is
//! the lowest. New builtins are appended under their canonical path: the first one a
//! breadth-first walk from the roots reaches, with keys in sorted order.
use crate::PushOut;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::bytecode::intrinsics;
use crate::heap::{Atom, Heap, Property};
use crate::interp::realm::{Intrinsic, installed_intrinsic, intrinsic};
use crate::value::{Obj, Value};

/// What an entry names.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum EntryKind {
    /// An object or symbol of the base realm, by path.
    Value,
    /// An entry of the intrinsics table, by name.
    Intrinsic,
}

impl EntryKind {
    fn as_str(self) -> &'static str {
        match self {
            EntryKind::Value => "value",
            EntryKind::Intrinsic => "intrinsic",
        }
    }
}

/// One line of the registry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub id: u32,
    pub kind: EntryKind,
    pub path: String,
    /// A tombstone: why the builtin went away. Its id is never used again.
    pub removed: Option<String>,
}

/// The registry.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Registry {
    /// In id order: `entries[i].id == i`.
    pub entries: Vec<Entry>,
}

/// What [`Registry::update`] changed.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Update {
    pub added: Vec<u32>,
    pub removed: Vec<u32>,
}

/// The header of the registry file.
const HEADER: &str = "\
# The builtin registry (D2371): stable ids for builtins, append-only. Never edit by hand:
# `scripts/registry-update.sh` appends new builtins and tombstones removed ones. Columns:
# id, kind (value: an object or symbol by path; intrinsic: by name), path, and for a
# tombstone why it was removed. An id is never renumbered or reused (the gate checks it).
";

impl Registry {
    /// Parses the registry file. Ids must be 0, 1, 2, ... in order, and no two live entries
    /// of a kind may share a path.
    pub fn parse(text: &str) -> Result<Registry, String> {
        let mut entries = Vec::new();
        let mut live = BTreeSet::new();
        for (n, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            if !(3..=4).contains(&cols.len()) {
                return Err(format!("line {}: expected 3 or 4 columns", n + 1));
            }
            let id: u32 = cols[0]
                .parse()
                .map_err(|_| format!("line {}: bad id {:?}", n + 1, cols[0]))?;
            if id as usize != entries.len() {
                return Err(format!(
                    "line {}: id {id} out of sequence (expected {})",
                    n + 1,
                    entries.len()
                ));
            }
            let kind = match cols[1] {
                "value" => EntryKind::Value,
                "intrinsic" => EntryKind::Intrinsic,
                k => return Err(format!("line {}: unknown kind {k:?}", n + 1)),
            };
            let removed = match cols.get(3) {
                None => None,
                Some(r) => Some(
                    r.strip_prefix("removed: ")
                        .ok_or_else(|| format!("line {}: expected `removed: ...`", n + 1))?
                        .to_string(),
                ),
            };
            if removed.is_none() && !live.insert((kind, cols[2].to_string())) {
                return Err(format!("line {}: {} registered twice", n + 1, cols[2]));
            }
            entries.push_out(Entry {
                id,
                kind,
                path: cols[2].to_string(),
                removed,
            });
        }
        Ok(Registry { entries })
    }

    /// The file text.
    pub fn to_text(&self) -> String {
        let mut out = String::from(HEADER);
        for e in &self.entries {
            out.push_str(&format!("{}\t{}\t{}", e.id, e.kind.as_str(), e.path));
            if let Some(r) = &e.removed {
                out.push_str("\tremoved: ");
                out.push_str(r);
            }
            out.push_out('\n');
        }
        out
    }

    /// The live entry with this kind and path.
    pub fn id_of(&self, kind: EntryKind, path: &str) -> Option<u32> {
        self.entries
            .iter()
            .find(|e| e.kind == kind && e.path == path && e.removed.is_none())
            .map(|e| e.id)
    }

    pub fn entry(&self, id: u32) -> Option<&Entry> {
        self.entries.get(id as usize)
    }

    /// Registry ids of the intrinsics table, by intrinsic id (`None`: unregistered).
    pub fn intrinsic_ids(&self) -> Vec<Option<u32>> {
        intrinsics::names()
            .map(|i| self.id_of(EntryKind::Intrinsic, i))
            .collect()
    }

    /// The ids of the builtin objects and symbols of `heap`, by cell offset: every live
    /// value entry resolved on this heap, the lowest id for an object with several paths.
    pub fn name_cells(&self, heap: &Heap) -> BTreeMap<u32, u32> {
        let mut r = Resolver::new(heap);
        let mut out = BTreeMap::new();
        for e in &self.entries {
            if e.kind == EntryKind::Value
                && e.removed.is_none()
                && let Some(cell) = r.resolve(&e.path).and_then(cell_of)
            {
                out.entry(cell).or_insert(e.id);
            }
        }
        out
    }

    /// What is wrong with the registry on this realm: live entries that no longer resolve,
    /// builtins without an entry, intrinsics without an entry. Empty when it is current.
    pub fn check(&self, heap: &Heap) -> Vec<String> {
        self.check_build(heap, &BTreeSet::new())
    }

    /// [`check`](Self::check) for a build that leaves out builtin families (the slim
    /// build, D2411): the live value entries in `absent` must not resolve, all others must.
    pub fn check_build(&self, heap: &Heap, absent: &BTreeSet<u32>) -> Vec<String> {
        let mut problems = Vec::new();
        let mut r = Resolver::new(heap);
        for e in self.entries.iter().filter(|e| e.removed.is_none()) {
            let ok = match e.kind {
                EntryKind::Value => r.resolve(&e.path).and_then(cell_of).is_some(),
                EntryKind::Intrinsic => intrinsics::names().any(|i| i == e.path),
            };
            if ok && absent.contains(&e.id) {
                problems.push(format!(
                    "{} {} (id {}) is absent from this build but resolves",
                    e.kind.as_str(),
                    e.path,
                    e.id
                ));
            } else if !ok && !absent.contains(&e.id) {
                problems.push(format!(
                    "{} {} (id {}) is gone",
                    e.kind.as_str(),
                    e.path,
                    e.id
                ));
            }
        }
        let named = self.name_cells(heap);
        for (cell, path) in canonical_names(heap) {
            if !named.contains_key(&cell) {
                problems.push_out(format!("value {path} has no entry"));
            }
        }
        for i in intrinsics::names() {
            if self.id_of(EntryKind::Intrinsic, i).is_none() {
                problems.push_out(format!("intrinsic {i} has no entry"));
            }
        }
        problems
    }

    /// The live value entries that don't resolve on this realm: what a build without some
    /// builtin families lacks (D2411).
    pub fn unresolved(&self, heap: &Heap) -> Vec<u32> {
        let mut r = Resolver::new(heap);
        self.entries
            .iter()
            .filter(|e| e.kind == EntryKind::Value && e.removed.is_none())
            .filter(|e| r.resolve(&e.path).and_then(cell_of).is_none())
            .map(|e| e.id)
            .collect()
    }

    /// Brings the registry up to date with this realm: tombstones live entries that no
    /// longer resolve (with `reason`), then appends the builtins and intrinsics that have
    /// none. Never renumbers.
    pub fn update(&mut self, heap: &Heap, reason: &str) -> Update {
        let mut u = Update::default();
        let mut r = Resolver::new(heap);
        for e in self.entries.iter_mut().filter(|e| e.removed.is_none()) {
            let ok = match e.kind {
                EntryKind::Value => r.resolve(&e.path).and_then(cell_of).is_some(),
                EntryKind::Intrinsic => intrinsics::names().any(|i| i == e.path),
            };
            if !ok {
                e.removed = Some(reason.to_string());
                u.removed.push_out(e.id);
            }
        }
        let named = self.name_cells(heap);
        for (cell, path) in canonical_names(heap) {
            if !named.contains_key(&cell) {
                u.added.push_out(self.push(EntryKind::Value, path));
            }
        }
        for i in intrinsics::names() {
            if self.id_of(EntryKind::Intrinsic, i).is_none() {
                u.added
                    .push_out(self.push(EntryKind::Intrinsic, i.to_string()));
            }
        }
        u
    }

    fn push(&mut self, kind: EntryKind, path: String) -> u32 {
        let id = self.entries.len() as u32;
        self.entries.push_out(Entry {
            id,
            kind,
            path,
            removed: None,
        });
        id
    }
}

/// The heap cell a builtin value is: an object's or a symbol's offset.
fn cell_of(v: Value) -> Option<u32> {
    v.as_object()
        .map(Obj::offset)
        .or_else(|| v.as_symbol().map(|s| s.offset()))
}

/// The realm intrinsics that serve as roots, with their names: the ones that hold builtin
/// objects. `Proto` reads `Prototype`, as in the spec.
pub const REALM_ROOTS: &[(Intrinsic, &str)] = &[
    (Intrinsic::ObjectProto, "ObjectPrototype"),
    (Intrinsic::FunctionProto, "FunctionPrototype"),
    (Intrinsic::ArrayProto, "ArrayPrototype"),
    (Intrinsic::ErrorProto, "ErrorPrototype"),
    (Intrinsic::TypeErrorProto, "TypeErrorPrototype"),
    (Intrinsic::RangeErrorProto, "RangeErrorPrototype"),
    (Intrinsic::ReferenceErrorProto, "ReferenceErrorPrototype"),
    (Intrinsic::SyntaxErrorProto, "SyntaxErrorPrototype"),
    (Intrinsic::InternalErrorProto, "InternalErrorPrototype"),
    (Intrinsic::OutOfMemoryError, "OutOfMemoryError"),
    (Intrinsic::EvalErrorProto, "EvalErrorPrototype"),
    (Intrinsic::UriErrorProto, "URIErrorPrototype"),
    (Intrinsic::AggregateErrorProto, "AggregateErrorPrototype"),
    (Intrinsic::BooleanProto, "BooleanPrototype"),
    (Intrinsic::NumberProto, "NumberPrototype"),
    (Intrinsic::StringProto, "StringPrototype"),
    (Intrinsic::SymbolProto, "SymbolPrototype"),
    (Intrinsic::SymbolRegistry, "SymbolRegistry"),
    (Intrinsic::IteratorProto, "IteratorPrototype"),
    (Intrinsic::ArrayIteratorProto, "ArrayIteratorPrototype"),
    (Intrinsic::StringIteratorProto, "StringIteratorPrototype"),
    (Intrinsic::MapProto, "MapPrototype"),
    (Intrinsic::SetProto, "SetPrototype"),
    (Intrinsic::WeakMapProto, "WeakMapPrototype"),
    (Intrinsic::WeakSetProto, "WeakSetPrototype"),
    (Intrinsic::MapIteratorProto, "MapIteratorPrototype"),
    (Intrinsic::SetIteratorProto, "SetIteratorPrototype"),
    (Intrinsic::PromiseProto, "PromisePrototype"),
    (Intrinsic::DateProto, "DatePrototype"),
    (Intrinsic::GeneratorProto, "GeneratorPrototype"),
    (Intrinsic::ArrayBufferProto, "ArrayBufferPrototype"),
    (Intrinsic::TypedArrayCtor, "TypedArray"),
    (Intrinsic::TypedArrayProto, "TypedArrayPrototype"),
    (Intrinsic::Int8ArrayProto, "Int8ArrayPrototype"),
    (Intrinsic::Uint8ArrayProto, "Uint8ArrayPrototype"),
    (
        Intrinsic::Uint8ClampedArrayProto,
        "Uint8ClampedArrayPrototype",
    ),
    (Intrinsic::Int16ArrayProto, "Int16ArrayPrototype"),
    (Intrinsic::Uint16ArrayProto, "Uint16ArrayPrototype"),
    (Intrinsic::Int32ArrayProto, "Int32ArrayPrototype"),
    (Intrinsic::Uint32ArrayProto, "Uint32ArrayPrototype"),
    (Intrinsic::Float16ArrayProto, "Float16ArrayPrototype"),
    (Intrinsic::Float32ArrayProto, "Float32ArrayPrototype"),
    (Intrinsic::Float64ArrayProto, "Float64ArrayPrototype"),
    (Intrinsic::BigInt64ArrayProto, "BigInt64ArrayPrototype"),
    (Intrinsic::BigUint64ArrayProto, "BigUint64ArrayPrototype"),
    (Intrinsic::DataViewProto, "DataViewPrototype"),
    (
        Intrinsic::GeneratorFunctionProto,
        "GeneratorFunctionPrototype",
    ),
    (Intrinsic::AsyncFunctionProto, "AsyncFunctionPrototype"),
    (
        Intrinsic::AsyncGeneratorFunctionProto,
        "AsyncGeneratorFunctionPrototype",
    ),
    (Intrinsic::AsyncIteratorProto, "AsyncIteratorPrototype"),
    (Intrinsic::AsyncGeneratorProto, "AsyncGeneratorPrototype"),
    (
        Intrinsic::AsyncFromSyncIteratorProto,
        "AsyncFromSyncIteratorPrototype",
    ),
    (Intrinsic::BigIntProto, "BigIntPrototype"),
    (Intrinsic::RegExpProto, "RegExpPrototype"),
    (
        Intrinsic::RegExpStringIteratorProto,
        "RegExpStringIteratorPrototype",
    ),
    (Intrinsic::IteratorHelperProto, "IteratorHelperPrototype"),
    (
        Intrinsic::WrapForValidIteratorProto,
        "WrapForValidIteratorPrototype",
    ),
    (Intrinsic::DisposableStackProto, "DisposableStackPrototype"),
    (
        Intrinsic::AsyncDisposableStackProto,
        "AsyncDisposableStackPrototype",
    ),
    (Intrinsic::SuppressedErrorProto, "SuppressedErrorPrototype"),
    (Intrinsic::WeakRefProto, "WeakRefPrototype"),
    (
        Intrinsic::FinalizationRegistryProto,
        "FinalizationRegistryPrototype",
    ),
    (Intrinsic::CallSiteProto, "CallSitePrototype"),
];

/// Every builtin object and symbol a walk from the roots reaches, each with its canonical
/// path (the first path breadth-first, keys in sorted order), in walk order.
pub fn canonical_names(heap: &Heap) -> Vec<(u32, String)> {
    names(heap, prelude_functions)
}

/// The own data properties of `heap`'s global object with string keys, as a dump compares
/// a VM's global object with its base's (`vm::dump::BaseNames::globals`): the key's atom (a
/// VM's base atoms are the base's), name, the value's bits, the property's flags. An
/// accessor has `u64::MAX` (no value has those bits).
pub fn base_globals(heap: &Heap) -> Vec<(u32, String, u64, u8)> {
    let Some(g) = intrinsic(heap, Intrinsic::Global).as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for k in heap.own_keys(g) {
        if heap.atom_symbol(k).is_some() {
            continue;
        }
        let name = heap.atom_name(k);
        match heap.get_own_property(g, k) {
            Some(Property::Data { value, flags }) => {
                out.push_out((k.id(), name, value.to_bits(), flags.bits()));
            }
            Some(_) => out.push_out((k.id(), name, u64::MAX, 0)),
            None => {}
        }
    }
    out
}

/// [`canonical_names`] without the last pass (the prelude's internal functions, whose
/// names need the prelude's source compiled): the same paths for everything else. For
/// `Vm::dump`, which refuses those functions anyway and should not link the prelude source.
pub fn reachable_names(heap: &Heap) -> Vec<(u32, String)> {
    names(heap, |_| Vec::new())
}

fn names(heap: &Heap, prelude: impl FnOnce(&Heap) -> Vec<(Value, String)>) -> Vec<(u32, String)> {
    // Cells are 8-byte aligned: one bit per word of the heap.
    let mut seen = Seen(alloc::vec![0; heap.bytes().len() / 512 + 1]);
    let mut out: Vec<(Value, String)> = Vec::new();
    let global = intrinsic(heap, Intrinsic::Global);
    if let Some(c) = cell_of(global) {
        seen.insert(c);
        out.push_out((global, String::from("globalThis")));
    }
    // Passes, so that a builtin gets the most readable path it has: what the globals
    // reach by properties (breadth-first), then the realm roots (`%TypedArray%` rather than
    // `Int8Array.[[Prototype]]`), then the prelude functions registered for intrinsics,
    // and last what only `[[Prototype]]` links reach from any of them.
    let mut globals = Vec::new();
    if let Some(g) = global.as_object() {
        for (seg, v, _) in edges(heap, g, false, None) {
            // A global's path is its name; keys that aren't identifiers keep `globalThis`.
            let p = child_path("globalThis", &seg);
            let p = match p.strip_prefix("globalThis.") {
                Some(name) => String::from(name),
                None => p,
            };
            globals.push_out((v, p));
        }
    }
    walk(heap, globals, false, &mut seen, &mut out);
    let realm = REALM_ROOTS
        .iter()
        .map(|(i, name)| (intrinsic(heap, *i), cat!("%", *name, "%")))
        .collect();
    walk(heap, realm, false, &mut seen, &mut out);
    let installed = intrinsics::names()
        .enumerate()
        .map(|(id, name)| {
            (
                installed_intrinsic(heap, id as u16),
                cat!("%intrinsic:", name, "%"),
            )
        })
        .collect();
    walk(heap, installed, false, &mut seen, &mut out);
    let named = out.clone();
    walk(heap, named, true, &mut seen, &mut out);
    // Last, the prelude's internal functions, which no property reaches (and what their
    // properties lead to).
    walk(heap, prelude(heap), true, &mut seen, &mut out);
    out.into_iter()
        .filter_map(|(v, p)| Some((cell_of(v)?, p)))
        .collect()
}

/// The cells a naming walk has seen: a bit per 8-byte word of the heap.
struct Seen(Vec<u64>);

impl Seen {
    fn contains(&self, cell: u32) -> bool {
        let w = (cell / 8) as usize;
        self.0
            .get(w / 64)
            .is_some_and(|x| x & (1u64 << (w % 64)) != 0)
    }

    /// Marks `cell`; whether it was new.
    fn insert(&mut self, cell: u32) -> bool {
        let w = (cell / 8) as usize;
        let (i, bit) = (w / 64, 1u64 << (w % 64));
        if i >= self.0.len() {
            self.0.resize(i + 1, 0);
        }
        let new = self.0[i] & bit == 0;
        self.0[i] |= bit;
        new
    }
}

/// Breadth-first from `roots`, adding the cells not `seen` yet to `out` with their paths.
/// A root that was seen before is still walked from (the last pass starts from everything
/// named so far).
fn walk(
    heap: &Heap,
    roots: Vec<(Value, String)>,
    proto: bool,
    seen: &mut Seen,
    out: &mut Vec<(Value, String)>,
) {
    let mut queue: Vec<(Value, String, bool, bool)> =
        roots.into_iter().map(|(v, p)| (v, p, true, true)).collect();
    let mut head = 0;
    while head < queue.len() {
        let e = &mut queue[head];
        let (v, path, expand, root) = (e.0, core::mem::take(&mut e.1), e.2, e.3);
        head += 1;
        let Some(c) = cell_of(v) else { continue };
        if seen.insert(c) {
            out.push_out((v, path.clone()));
        } else if !root {
            continue;
        }
        let Some(o) = v.as_object().filter(|_| expand) else {
            continue;
        };
        // Children seen already are skipped when popped (only roots are walked again), so
        // they are left out here, before their paths are made: most edges of the later
        // passes lead to named builtins (0184: this halved a dump's fixed cost).
        for (seg, child, child_expand) in edges(heap, o, proto, Some(seen)) {
            queue.push_out((child, child_path(&path, &seg), child_expand, false));
        }
    }
}

/// The prelude function table's entries (D2440) with their paths, `%prelude:Name%`. The
/// names are the source names of this build's prelude, which the compiler gives in slot
/// order; a heap of another build may hold other functions there.
fn prelude_functions(heap: &Heap) -> Vec<(Value, String)> {
    let Some(t) = intrinsic(heap, Intrinsic::PreludeFunctions).as_internal() else {
        return Vec::new();
    };
    let Ok(mut scratch) = Heap::new() else {
        return Vec::new();
    };
    let opts = crate::compiler::CompileOptions::privileged_script("<prelude>");
    let Ok(names) =
        crate::compiler::prelude_function_names(&mut scratch, super::PRELUDE.as_bytes(), &opts)
    else {
        return Vec::new();
    };
    names
        .iter()
        .enumerate()
        .filter(|&(i, _)| (i as u32) < heap.value_array_len(t))
        .map(|(i, &n)| {
            let v = heap.value_array_get(t, i as u32);
            (v, format!("%prelude:{}%", scratch.atom_name(n)))
        })
        .collect()
}

/// `path` followed by `seg`; an accessor segment's `get `/`set ` goes to the front.
fn child_path(path: &str, seg: &str) -> String {
    if let Some(rest) = seg.strip_prefix("get ") {
        cat!("get ", path, rest)
    } else if let Some(rest) = seg.strip_prefix("set ") {
        cat!("set ", path, rest)
    } else {
        cat!(path, seg)
    }
}

/// The outgoing edges of `o` that lead to heap values: (segment, value, whether to walk on
/// from it). Accessor segments carry their `get `/`set ` prefix; their functions are not
/// walked (a path can only name them as a whole).
/// With `seen`, only the edges to cells not seen yet.
fn edges(heap: &Heap, o: Obj, proto: bool, seen: Option<&Seen>) -> Vec<(String, Value, bool)> {
    let new = |v: Value| cell_of(v).is_some_and(|c| seen.is_none_or(|s| !s.contains(c)));
    let mut out: Vec<(String, Value, bool)> = Vec::new();
    for k in heap.own_keys(o) {
        let (a, b) = match heap.get_own_property(o, k) {
            Some(Property::Data { value, .. }) => (value, Value::UNDEFINED),
            Some(Property::Accessor { getter, setter, .. }) => (getter, setter),
            None => continue,
        };
        if !new(a) && !new(b) {
            continue;
        }
        let Some(seg) = segment(heap, k) else {
            continue;
        };
        match heap.get_own_property(o, k) {
            Some(Property::Data { value, .. }) => out.push_out((seg, value, true)),
            Some(Property::Accessor { getter, setter, .. }) => {
                if new(getter) {
                    out.push_out((cat!("get ", seg), getter, false));
                }
                if new(setter) {
                    out.push_out((cat!("set ", seg), setter, false));
                }
            }
            None => {}
        }
    }
    crate::sort::sort_by(&mut out, |a, b| a.0 < b.0);
    if proto
        && let Some(p) = heap.get_prototype_of(o)
        && new(Value::object(p))
    {
        out.push_out((String::from(".[[Prototype]]"), Value::object(p), true));
    }
    out
}

/// A key as a path segment: `.name`, `["odd key"]`, `[Symbol.iterator]`; `None` for keys
/// a path cannot name (symbols without a `Symbol.` description).
fn segment(heap: &Heap, k: Atom) -> Option<String> {
    if let Some(s) = heap.atom_symbol(k) {
        let d = heap.symbol_description(s).as_string()?;
        let d = heap.str_to_string(d);
        return d.starts_with("Symbol.").then(|| cat!("[", d, "]"));
    }
    let name = heap.atom_name(k);
    let ident = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    Some(if ident {
        cat!(".", name)
    } else {
        cat!("[", quote(&name), "]")
    })
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push_out(c),
        }
    }
    out.push_out('"');
    out
}

/// Resolves paths on one heap, caching each object's segments.
struct Resolver<'h> {
    heap: &'h Heap,
    roots: BTreeMap<String, Value>,
    cache: BTreeMap<u32, Vec<(String, Value)>>,
}

/// The value at a builtin path as [`reachable_names`] writes it (no `%prelude:` roots),
/// for the dump loader's `$dump.builtin` (0182); `None` if a step is missing.
pub fn resolve_reachable(heap: &Heap, path: &str) -> Option<Value> {
    Resolver::with_prelude(heap, Vec::new()).resolve(path)
}

impl<'h> Resolver<'h> {
    fn new(heap: &'h Heap) -> Resolver<'h> {
        Resolver::with_prelude(heap, prelude_functions(heap))
    }

    fn with_prelude(heap: &'h Heap, prelude: Vec<(Value, String)>) -> Resolver<'h> {
        let mut roots = BTreeMap::new();
        let global = intrinsic(heap, Intrinsic::Global);
        roots.insert(String::from("globalThis"), global);
        for (i, name) in REALM_ROOTS {
            roots.insert(format!("%{name}%"), intrinsic(heap, *i));
        }
        for (id, name) in intrinsics::names().enumerate() {
            let v = installed_intrinsic(heap, id as u16);
            if !v.is_undefined() {
                roots.insert(format!("%intrinsic:{name}%"), v);
            }
        }
        for (v, path) in prelude {
            roots.insert(path, v);
        }
        Resolver {
            heap,
            roots,
            cache: BTreeMap::new(),
        }
    }

    fn segments(&mut self, o: Obj) -> &Vec<(String, Value)> {
        let heap = self.heap;
        self.cache.entry(o.offset()).or_insert_with(|| {
            let mut out = Vec::new();
            for k in heap.own_keys(o) {
                let Some(seg) = segment(heap, k) else {
                    continue;
                };
                match heap.get_own_property(o, k) {
                    Some(Property::Data { value, .. }) => out.push_out((seg, value)),
                    Some(Property::Accessor { getter, setter, .. }) => {
                        out.push_out((format!("get {seg}"), getter));
                        out.push_out((format!("set {seg}"), setter));
                    }
                    None => {}
                }
            }
            out
        })
    }

    /// The value at `path`, or `None` if a step is missing.
    fn resolve(&mut self, path: &str) -> Option<Value> {
        let (accessor, path) = match path.get(..4) {
            Some("get ") | Some("set ") => (Some(&path[..3]), &path[4..]),
            _ => (None, path),
        };
        let (root, rest) = split_root(path);
        let mut segs = Vec::new();
        let mut rest_iter = rest;
        while !rest_iter.is_empty() {
            let (seg, tail) = split_segment(rest_iter)?;
            segs.push_out(seg);
            rest_iter = tail;
        }
        // The accessor prefix belongs to the last property segment.
        let accessor_at = segs.iter().rposition(|s| *s != ".[[Prototype]]");
        let mut cur = match self.roots.get(root) {
            Some(v) => *v,
            None => {
                let g = intrinsic(self.heap, Intrinsic::Global).as_object()?;
                let seg = format!(".{root}");
                match self.segments(g).iter().find(|s| s.0 == seg) {
                    Some(s) => s.1,
                    // A realm prototype whose constructor this build leaves out
                    // (`Iterator.prototype` in the slim build, D2411) keeps its id.
                    None if segs.first() == Some(&".prototype") => {
                        let p = Intrinsic::prototype_of(root)?;
                        segs.remove(0);
                        intrinsic(self.heap, p)
                    }
                    None => return None,
                }
            }
        };
        for (n, seg) in segs.into_iter().enumerate() {
            let o = cur.as_object()?;
            if seg == ".[[Prototype]]" {
                cur = Value::object(self.heap.get_prototype_of(o)?);
                continue;
            }
            let want = match accessor {
                Some(a) if accessor_at == Some(n) => format!("{a} {seg}"),
                _ => seg.to_string(),
            };
            cur = self.segments(o).iter().find(|s| s.0 == want)?.1;
        }
        Some(cur)
    }
}

/// `"%Foo%.bar"` -> (`"%Foo%"`, `".bar"`); `"Array.prototype"` -> (`"Array"`, `".prototype"`).
fn split_root(path: &str) -> (&str, &str) {
    if let Some(rest) = path.strip_prefix('%')
        && let Some(end) = rest.find('%')
    {
        return path.split_at(end + 2);
    }
    let end = path.find(['.', '[']).unwrap_or(path.len());
    path.split_at(end)
}

/// The first segment of `rest` and what follows it.
fn split_segment(rest: &str) -> Option<(&str, &str)> {
    if rest.starts_with(".[[Prototype]]") {
        return Some(rest.split_at(".[[Prototype]]".len()));
    }
    if let Some(r) = rest.strip_prefix('.') {
        let end = r.find(['.', '[']).unwrap_or(r.len());
        return Some(rest.split_at(end + 1));
    }
    if rest.starts_with("[\"") {
        // A quoted key: up to the closing `"]`, skipping escapes.
        let b = rest.as_bytes();
        let mut i = 2;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'"' if b.get(i + 1) == Some(&b']') => return Some(rest.split_at(i + 2)),
                _ => i += 1,
            }
        }
        return None;
    }
    if rest.starts_with('[') {
        let end = rest.find(']')?;
        return Some(rest.split_at(end + 1));
    }
    None
}

#[cfg(test)]
mod tests;
