//! ES module records, linking and namespace objects (D3171, docs/compiler.md 5.11).
//!
//! A module record is an internal [`ObjectClass::Record`] (slots in [`slot`]) in the
//! realm's registry (`Intrinsic::Modules`), so collections and images carry linked and
//! evaluated graphs like any other heap state. Its environment is its body closure: the
//! module template's captures are its top-level bindings in Boxes (compiler/module.rs),
//! which linking creates. An import's capture is the exporting module's Box, so imports
//! are live bindings without any indirection.
//!
//! Loading (`vm/module.rs`) and linking run no user code and are Rust; evaluation calls
//! the module bodies and is builtins/module.ts. Namespace objects are Proxies over the
//! traps of module.ts, so that no property path can skip their exotic behavior.

use alloc::vec::Vec;

use super::conv::Fault;
use super::realm::{self, ErrorKind, Intrinsic, intrinsic_obj};
use crate::bytecode::template;
use crate::compiler::module_meta::{ENV_FUNCTION, ENV_LEXICAL, ENV_META, ENV_VAR, ModuleMeta};
use crate::heap::cells::{BoxCell, CellKind};
use crate::heap::{Atom, Heap, HeapError, ObjectClass, PropFlags, PropertyDescriptor, Ref, atoms};
use crate::value::{Obj, Value};

/// The slots of a module record (builtins/module.ts uses the same numbers).
#[allow(dead_code)]
pub(crate) mod slot {
    /// The module's key, a string.
    pub const KEY: u32 = 0;
    /// [`super::status`], an int.
    pub const STATUS: u32 = 1;
    /// The module closure: its template is the body, its captures the environment.
    pub const BODY: u32 = 2;
    /// A Record of the requested module records (by request index), once loaded.
    pub const REQUESTED: u32 = 3;
    /// The number of requests, an int.
    pub const COUNT: u32 = 4;
    /// The evaluation error of an `ERRORED` module.
    pub const ERROR: u32 = 5;
    /// [[DFSIndex]] and [[DFSAncestorIndex]] while it evaluates (module.ts).
    pub const DFS: u32 = 6;
    pub const ANCESTOR: u32 = 7;
    /// The namespace object, once made.
    pub const NAMESPACE: u32 = 8;
    /// The metadata (compiler/module_meta.rs), an internal U32Array.
    pub const META: u32 = 9;
    /// The record under it on the evaluation stack (module.ts), or null.
    pub const STACK: u32 = 10;
    /// A Box holding the namespace (what `import * as` and `export * as` bind), once made.
    pub const NS_BOX: u32 = 11;
    /// [`super::flags`], an int.
    pub const FLAGS: u32 = 12;
    /// [[CycleRoot]] (module.ts): the root of its strongly connected component once that
    /// is done, or null.
    pub const CYCLE_ROOT: u32 = 13;
    /// [[AsyncEvaluationOrder]]: 0 unset, -1 done, else the order (an int).
    pub const ASYNC_ORDER: u32 = 14;
    /// [[PendingAsyncDependencies]], an int.
    pub const PENDING: u32 = 15;
    /// [[AsyncParentModules]]: an Array of records, or null.
    pub const PARENTS: u32 = 16;
    /// [[TopLevelCapability]]: the promise of an Evaluate of it, or undefined.
    pub const CAPABILITY: u32 = 17;
    pub const COUNT_SLOTS: u32 = 18;
}

/// `slot::FLAGS` of a module record.
pub(crate) mod flags {
    /// A JSON module (a synthetic module with the parsed text as its default export).
    pub const JSON: i32 = 1;
    /// [[HasTLA]]: the body is an async function (top-level await).
    pub const TLA: i32 = 2;
}

/// [[Status]] of a module record (module.ts tests these numbers).
#[allow(dead_code)]
pub(crate) mod status {
    pub const NEW: i32 = 0;
    pub const LINKING: i32 = 1;
    pub const LINKED: i32 = 2;
    pub const EVALUATING: i32 = 3;
    pub const EVALUATED: i32 = 4;
    /// Evaluated with an error ([[EvaluationError]] in `slot::ERROR`).
    pub const ERRORED: i32 = 5;
    /// evaluating-async: its SCC is done, an async module in it (or below) is not.
    pub const EVALUATING_ASYNC: i32 = 6;
}

/// Slots of a namespace's proxy target (an ordinary object with internal slots).
mod ns {
    /// The module record.
    pub const MODULE: u32 = 0;
    /// A hidden object from export name to its index in `BOXES` (an int).
    pub const INDEX: u32 = 1;
    /// An internal ValueArray of the bindings' Boxes, in key order.
    pub const BOXES: u32 = 2;
    /// An internal U32Array of the export names (atoms), sorted by code units.
    pub const KEYS: u32 = 3;
    pub const COUNT: u32 = 4;
}

/// The slots of the `Intrinsic::Modules` Record.
mod modules_slot {
    /// The registry: a null-prototype object from key atom to module record.
    pub const REGISTRY: u32 = 0;
    /// The handler of every namespace proxy.
    pub const TRAPS: u32 = 1;
    /// The async host function `import()` calls (fn id `vm::IMPORT_FN_ID`), or undefined:
    /// then `import()` rejects as without modules (builtins/import.ts).
    pub const IMPORT: u32 = 2;
    /// The last [[AsyncEvaluationOrder]] given (IncrementModuleAsyncEvaluationCount), an
    /// int.
    pub const ASYNC_COUNT: u32 = 3;
    pub const COUNT: u32 = 4;
}

/// `Intrinsic::Modules`: the Record of [`modules_slot`], made on first use.
fn modules(heap: &mut Heap) -> Result<Obj, HeapError> {
    if let Some(m) = realm::intrinsic(heap, Intrinsic::Modules).as_object() {
        return Ok(m);
    }
    let r = heap.alloc_object(ObjectClass::Record, None, modules_slot::COUNT)?;
    let registry = heap.new_object(None)?;
    // The handler of every namespace: builtin closures of `%ModuleOp` with the internal
    // method's number (builtins/module.ts).
    let traps = heap.new_object(None)?;
    let names = [
        "get",
        "getOwnPropertyDescriptor",
        "defineProperty",
        "ownKeys",
        "set",
    ];
    for (op, name) in names.into_iter().enumerate() {
        let key = heap.atom_str(name)?;
        let f = module_op_closure(heap, Value::int(op as i32), 0)?;
        realm::define_hidden(heap, traps, key, Value::object(f))?;
    }
    heap.set_object_slot(r, modules_slot::REGISTRY, Value::object(registry));
    heap.set_object_slot(r, modules_slot::TRAPS, Value::object(traps));
    heap.set_object_slot(r, modules_slot::IMPORT, Value::UNDEFINED);
    heap.set_object_slot(r, modules_slot::ASYNC_COUNT, Value::int(0));
    realm::set_intrinsic(heap, Intrinsic::Modules, Value::object(r));
    Ok(r)
}

/// A builtin closure of `%ModuleOp` with `data` (`this`: an op, or a Record of an op and
/// its operand, builtins/module.ts).
fn module_op_closure(heap: &mut Heap, data: Value, length: i32) -> Result<Obj, HeapError> {
    let proto = intrinsic_obj(heap, Intrinsic::FunctionProto);
    let class = ObjectClass::BuiltinClosure;
    let f = heap.alloc_object(class, Some(proto), class.slots())?;
    let id = crate::bytecode::intrinsics::intrinsic_id_of!("ModuleOp");
    heap.set_object_slot(f, 0, Value::int(i32::from(id)));
    heap.set_object_slot(f, 1, data);
    heap.set_object_slot(f, 2, Value::int(length));
    heap.set_lazy_builtin_closure_props(f);
    Ok(f)
}

fn registry(heap: &Heap) -> Option<Obj> {
    let m = realm::intrinsic(heap, Intrinsic::Modules).as_object()?;
    heap.object_slot(m, modules_slot::REGISTRY).as_object()
}

/// Sets the async host function `import()` calls (undefined: `import()` rejects).
pub(crate) fn set_import_function(heap: &mut Heap, f: Value) -> Result<(), HeapError> {
    let m = modules(heap)?;
    heap.set_object_slot(m, modules_slot::IMPORT, f);
    Ok(())
}

/// The async host function `import()` calls, or undefined.
fn import_function(heap: &Heap) -> Value {
    match realm::intrinsic(heap, Intrinsic::Modules).as_object() {
        Some(m) => heap.object_slot(m, modules_slot::IMPORT),
        None => Value::UNDEFINED,
    }
}

/// The record of the module with key `key`, if loaded.
pub(crate) fn find(heap: &mut Heap, key: &str) -> Result<Option<Obj>, HeapError> {
    let Some(reg) = registry(heap) else {
        return Ok(None);
    };
    let k = heap.atom_str(key)?;
    Ok(heap
        .get_own_property(reg, k)
        .and_then(|p| p.value())
        .and_then(|v| v.as_object()))
}

/// A new (unlinked) record for compiled module `template` with metadata `meta`,
/// registered under `key`.
pub(crate) fn new_record(
    heap: &mut Heap,
    key: &str,
    template: Ref,
    meta: Ref,
) -> Result<Obj, Fault> {
    let mods = modules(heap)?;
    let reg = heap
        .object_slot(mods, modules_slot::REGISTRY)
        .as_object()
        .expect("module registry");
    let r = heap.alloc_object(ObjectClass::Record, None, slot::COUNT_SLOTS)?;
    for i in 0..slot::COUNT_SLOTS {
        heap.set_object_slot(r, i, Value::UNDEFINED);
    }
    let k = heap.alloc_str(key)?;
    heap.set_object_slot(r, slot::KEY, Value::string(k));
    heap.set_object_slot(r, slot::STATUS, Value::int(status::NEW));
    let async_body = u32::from(
        heap.arena()
            .get::<u16>(template.0, crate::heap::cells::FuncTemplate::FLAGS),
    ) & template::func_flags::ASYNC
        != 0;
    let fl = if async_body { flags::TLA } else { 0 };
    heap.set_object_slot(r, slot::FLAGS, Value::int(fl));
    heap.set_object_slot(r, slot::META, Value::internal(meta.0));
    init_evaluation_slots(heap, r);
    let body = super::run::alloc_closure_of(heap, template)?;
    let n = heap.object_slot_count(body);
    for i in 0..n {
        heap.set_object_slot(body, i, Value::UNDEFINED);
    }
    heap.set_object_slot(r, slot::BODY, Value::object(body));
    let atom = heap.intern(k)?;
    realm::define_hidden(heap, reg, atom, Value::object(r))?;
    Ok(r)
}

/// A new synthetic module (the JSON modules proposal's Synthetic Module Record) registered
/// under `key`, whose exports `names` hold `values`: it has no code, needs no linking and is
/// evaluated from the start, so importers bind its Boxes and read what they hold. Its body
/// is a Record of the Boxes instead of a closure, which marks it synthetic. `flags` its
/// [`flags`].
pub(crate) fn new_synthetic(
    heap: &mut Heap,
    key: &str,
    names: &[Atom],
    values: &[Value],
    flags: i32,
) -> Result<Obj, Fault> {
    let mods = modules(heap)?;
    let reg = heap
        .object_slot(mods, modules_slot::REGISTRY)
        .as_object()
        .expect("module registry");
    let mut meta = ModuleMeta::default();
    for (i, &n) in names.iter().enumerate() {
        meta.env.push((n, ENV_VAR, 0));
        meta.local.push((n, i as u32));
    }
    let words = meta.encode();
    let arr = heap.alloc_u32_array(words.len() as u32)?;
    for (i, &w) in words.iter().enumerate() {
        heap.u32_array_set(arr, i as u32, w);
    }
    let r = heap.alloc_object(ObjectClass::Record, None, slot::COUNT_SLOTS)?;
    for i in 0..slot::COUNT_SLOTS {
        heap.set_object_slot(r, i, Value::UNDEFINED);
    }
    let body = heap.alloc_object(ObjectClass::Record, None, names.len() as u32)?;
    for (i, &v) in values.iter().enumerate() {
        let b = new_box(heap, v)?;
        heap.set_object_slot(body, i as u32, b);
    }
    let k = heap.alloc_str(key)?;
    heap.set_object_slot(r, slot::KEY, Value::string(k));
    heap.set_object_slot(r, slot::STATUS, Value::int(status::EVALUATED));
    heap.set_object_slot(r, slot::FLAGS, Value::int(flags));
    heap.set_object_slot(r, slot::META, Value::internal(arr));
    init_evaluation_slots(heap, r);
    heap.set_object_slot(r, slot::BODY, Value::object(body));
    set_requested(heap, r, &[])?;
    let atom = heap.intern(k)?;
    realm::define_hidden(heap, reg, atom, Value::object(r))?;
    Ok(r)
}

/// The evaluation slots of a new record (module.ts reads them).
fn init_evaluation_slots(heap: &mut Heap, r: Obj) {
    heap.set_object_slot(r, slot::STACK, Value::NULL);
    heap.set_object_slot(r, slot::CYCLE_ROOT, Value::NULL);
    heap.set_object_slot(r, slot::ASYNC_ORDER, Value::int(0));
    heap.set_object_slot(r, slot::PENDING, Value::int(0));
    heap.set_object_slot(r, slot::PARENTS, Value::NULL);
}

/// Sets export `name` of synthetic module `m` (SetSyntheticModuleExport): importers see the
/// new value. `false` if `m` is not synthetic or has no export `name`.
pub(crate) fn set_synthetic_export(
    heap: &mut Heap,
    m: Obj,
    name: &str,
    v: Value,
) -> Result<bool, HeapError> {
    let body = heap.object_slot(m, slot::BODY).as_object();
    if body.is_none_or(|b| heap.object_class(b) != ObjectClass::Record) {
        return Ok(false);
    }
    let name = heap.atom_str(name)?;
    let Some(&(_, e)) = meta(heap, m).local.iter().find(|x| x.0 == name) else {
        return Ok(false);
    };
    let b = env_box(heap, m, e);
    heap.arena_mut()
        .set(b.as_internal().expect("a Box"), BoxCell::VALUE, v);
    Ok(true)
}

/// Forgets the module with key `key` (a graph whose load failed).
pub(crate) fn remove(heap: &mut Heap, key: &str) -> Result<(), HeapError> {
    if let Some(reg) = registry(heap) {
        let k = heap.atom_str(key)?;
        heap.delete_own_property(reg, k)?;
    }
    Ok(())
}

pub(crate) fn meta(heap: &Heap, m: Obj) -> ModuleMeta {
    let a = heap
        .object_slot(m, slot::META)
        .as_internal()
        .expect("module metadata");
    let words: Vec<u32> = (0..heap.u32_array_len(a))
        .map(|i| heap.u32_array_get(a, i))
        .collect();
    ModuleMeta::decode(&words).expect("module metadata")
}

/// The [`flags`] of `m`.
pub(crate) fn flags_of(heap: &Heap, m: Obj) -> i32 {
    heap.object_slot(m, slot::FLAGS).as_int().unwrap_or(0)
}

pub(crate) fn status_of(heap: &Heap, m: Obj) -> i32 {
    heap.object_slot(m, slot::STATUS)
        .as_int()
        .unwrap_or(status::NEW)
}

fn set_status(heap: &mut Heap, m: Obj, s: i32) {
    heap.set_object_slot(m, slot::STATUS, Value::int(s));
}

/// The record of request `i` of `m`.
fn requested(heap: &Heap, m: Obj, i: u32) -> Obj {
    let r = heap
        .object_slot(m, slot::REQUESTED)
        .as_object()
        .expect("requested modules");
    heap.object_slot(r, i)
        .as_object()
        .expect("requested module")
}

/// Sets the requested records of `m` (by request index).
pub(crate) fn set_requested(heap: &mut Heap, m: Obj, records: &[Obj]) -> Result<(), HeapError> {
    let r = heap.alloc_object(ObjectClass::Record, None, records.len() as u32)?;
    for (i, &x) in records.iter().enumerate() {
        heap.set_object_slot(r, i as u32, Value::object(x));
    }
    heap.set_object_slot(m, slot::REQUESTED, Value::object(r));
    heap.set_object_slot(m, slot::COUNT, Value::int(records.len() as i32));
    Ok(())
}

pub(crate) fn key_of(heap: &Heap, m: Obj) -> alloc::string::String {
    let v = heap.object_slot(m, slot::KEY);
    v.as_string()
        .map(|s| heap.str_to_string(s))
        .unwrap_or_default()
}

/// The Box of environment entry `i` of `m`.
fn env_box(heap: &Heap, m: Obj, i: u32) -> Value {
    let body = heap
        .object_slot(m, slot::BODY)
        .as_object()
        .expect("module body");
    heap.object_slot(body, i)
}

fn new_box(heap: &mut Heap, v: Value) -> Result<Value, HeapError> {
    let c = heap.alloc_cell(CellKind::BoxCell, BoxCell::LAYOUT.min_size)?;
    heap.arena_mut().set(c, BoxCell::VALUE, v);
    Ok(Value::internal(c))
}

fn box_value(heap: &Heap, b: Value) -> Value {
    heap.arena()
        .get(b.as_internal().expect("a Box"), BoxCell::VALUE)
}

/// What ResolveExport (16.2.1.6.3) finds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resolution {
    /// Environment entry `.1` of module `.0`.
    Binding(Obj, u32),
    /// The namespace of the module.
    Namespace(Obj),
    None,
    Ambiguous,
}

/// ResolveExport ( exportName [ , resolveSet ] ). Recursive along re-export chains, which
/// are as deep as the graph; `depth` stops a runaway one.
fn resolve_export(
    heap: &Heap,
    m: Obj,
    name: Atom,
    set: &mut Vec<(Obj, Atom)>,
    depth: u32,
) -> Result<Resolution, Fault> {
    if depth > 10_000 {
        return Err(Fault::error(
            ErrorKind::RangeError,
            "too many re-exports".into(),
        ));
    }
    if set.contains(&(m, name)) {
        // A circular import request.
        return Ok(Resolution::None);
    }
    set.push((m, name));
    let meta = meta(heap, m);
    if let Some(&(_, e)) = meta.local.iter().find(|e| e.0 == name) {
        return Ok(Resolution::Binding(m, e));
    }
    if let Some(&(_, req, import)) = meta.indirect.iter().find(|e| e.0 == name) {
        let target = requested(heap, m, req);
        if import.is_null() {
            return Ok(Resolution::Namespace(target));
        }
        return resolve_export(heap, target, import, set, depth + 1);
    }
    if name == atoms::DEFAULT {
        // A star export never provides `default`.
        return Ok(Resolution::None);
    }
    let mut star = Resolution::None;
    for &req in &meta.star {
        let target = requested(heap, m, req);
        match resolve_export(heap, target, name, set, depth + 1)? {
            Resolution::Ambiguous => return Ok(Resolution::Ambiguous),
            Resolution::None => {}
            r if star == Resolution::None => star = r,
            r if r != star => return Ok(Resolution::Ambiguous),
            _ => {}
        }
    }
    Ok(star)
}

/// GetExportedNames ( [ exportStarSet ] ), 16.2.1.6.2.
fn exported_names(heap: &Heap, m: Obj, seen: &mut Vec<Obj>, out: &mut Vec<Atom>) {
    if seen.contains(&m) {
        return;
    }
    seen.push(m);
    let meta = meta(heap, m);
    for &(n, _) in &meta.local {
        out.push(n);
    }
    for &(n, ..) in &meta.indirect {
        out.push(n);
    }
    for &req in &meta.star {
        let mut names = Vec::new();
        exported_names(heap, requested(heap, m, req), seen, &mut names);
        for n in names {
            if n != atoms::DEFAULT && !out.contains(&n) {
                out.push(n);
            }
        }
    }
}

fn link_error(heap: &Heap, m: Obj, name: Atom, ambiguous: bool) -> Fault {
    let key = key_of(heap, m);
    let name = heap.atom_name(name);
    let msg = if ambiguous {
        crate::text::cat!("export '", &name, "' in module '", &key, "' is ambiguous")
    } else {
        crate::text::cat!("Could not find export '", &name, "' in module '", &key, "'")
    };
    Fault::error(ErrorKind::SyntaxError, msg)
}

/// The Box a resolution binds.
fn resolution_box(heap: &mut Heap, r: Resolution) -> Result<Value, Fault> {
    match r {
        Resolution::Binding(m, e) => Ok(env_box(heap, m, e)),
        Resolution::Namespace(m) => namespace_box(heap, m),
        _ => unreachable!("an unresolved export has no Box"),
    }
}

/// Links the graph of `root` (Link(), 16.2.1.5.2, without async modules): every module
/// reachable from it that is not linked yet gets its environment, imports are bound and
/// the top-level functions are instantiated. A missing or ambiguous import or re-export
/// is a SyntaxError and leaves the modules of this link unlinked (`NEW`).
pub(crate) fn link(heap: &mut Heap, root: Obj) -> Result<(), Fault> {
    // The modules to link: reachable and still NEW.
    let mut todo = Vec::new();
    let mut stack = alloc::vec![root];
    while let Some(m) = stack.pop() {
        if status_of(heap, m) != status::NEW {
            continue;
        }
        set_status(heap, m, status::LINKING);
        todo.push(m);
        let n = heap.object_slot(m, slot::COUNT).as_int().unwrap_or(0) as u32;
        for i in 0..n {
            stack.push(requested(heap, m, i));
        }
    }
    let r = link_all(heap, &todo);
    let s = if r.is_ok() {
        status::LINKED
    } else {
        status::NEW
    };
    for &m in &todo {
        set_status(heap, m, s);
    }
    r
}

fn link_all(heap: &mut Heap, todo: &[Obj]) -> Result<(), Fault> {
    // 1. The Boxes of the modules' own bindings (InitializeEnvironment).
    for &m in todo {
        let meta = meta(heap, m);
        let body = heap
            .object_slot(m, slot::BODY)
            .as_object()
            .expect("module body");
        for (i, &(_, kind, _)) in meta.env.iter().enumerate() {
            let b = match kind {
                ENV_VAR => new_box(heap, Value::UNDEFINED)?,
                ENV_LEXICAL | ENV_FUNCTION => new_box(heap, Value::UNINITIALIZED)?,
                ENV_META => {
                    let meta_obj = import_meta(heap, m)?;
                    new_box(heap, Value::object(meta_obj))?
                }
                _ => continue,
            };
            heap.set_object_slot(body, i as u32, b);
        }
    }
    // 2. Imports and re-exports resolve; imports share the exporter's Box.
    for &m in todo {
        let meta = meta(heap, m);
        for &(name, req, import) in &meta.indirect {
            if import.is_null() {
                continue;
            }
            let target = requested(heap, m, req);
            match resolve_export(heap, target, import, &mut Vec::new(), 0)? {
                Resolution::None => return Err(link_error(heap, target, import, false)),
                Resolution::Ambiguous => return Err(link_error(heap, target, import, true)),
                _ => {}
            }
            let _ = name;
        }
        let body = heap
            .object_slot(m, slot::BODY)
            .as_object()
            .expect("module body");
        for &(e, req, import) in &meta.imports {
            let target = requested(heap, m, req);
            let r = if import.is_null() {
                Resolution::Namespace(target)
            } else {
                resolve_export(heap, target, import, &mut Vec::new(), 0)?
            };
            match r {
                Resolution::None => return Err(link_error(heap, target, import, false)),
                Resolution::Ambiguous => return Err(link_error(heap, target, import, true)),
                _ => {}
            }
            let b = resolution_box(heap, r)?;
            heap.set_object_slot(body, e, b);
        }
    }
    // 3. The top-level functions, whose captures are all environment entries.
    for &m in todo {
        let meta = meta(heap, m);
        let body = heap
            .object_slot(m, slot::BODY)
            .as_object()
            .expect("module body");
        let t = heap.closure_template(body);
        for (i, &(_, kind, child)) in meta.env.iter().enumerate() {
            if kind != ENV_FUNCTION {
                continue;
            }
            let ct = template::child(heap, t, child).expect("module function template");
            let f = super::run::alloc_closure_of(heap, ct)?;
            let ncap =
                heap.arena()
                    .get(ct.0, crate::heap::cells::FuncTemplate::CAPTURES) as u32;
            for c in 0..ncap {
                let d = template::capture_desc(heap, ct.0, c);
                let b = heap.object_slot(body, d & 0xFFFF);
                heap.set_object_slot(f, c, b);
            }
            super::run::function_properties(heap, f, ct.0)?;
            let b = heap.object_slot(body, i as u32);
            heap.arena_mut().set(
                b.as_internal().expect("a Box"),
                BoxCell::VALUE,
                Value::object(f),
            );
        }
    }
    Ok(())
}

/// The import.meta object of `m` (16.2.1.11's HostGetImportMetaProperties: `url`, the
/// module's key): a null-prototype ordinary object.
fn import_meta(heap: &mut Heap, m: Obj) -> Result<Obj, HeapError> {
    let o = heap.new_object(None)?;
    let key = heap.object_slot(m, slot::KEY);
    let url = heap.atom_str("url")?;
    heap.define_own_property(o, url, &PropertyDescriptor::data(key, PropFlags::ALL))?;
    Ok(o)
}

/// The Box holding the namespace of `m`.
fn namespace_box(heap: &mut Heap, m: Obj) -> Result<Value, Fault> {
    let b = heap.object_slot(m, slot::NS_BOX);
    if b.as_internal().is_some() {
        return Ok(b);
    }
    let b = new_box(heap, Value::UNINITIALIZED)?;
    heap.set_object_slot(m, slot::NS_BOX, b);
    let ns = namespace(heap, m)?;
    heap.arena_mut().set(
        b.as_internal().expect("a Box"),
        BoxCell::VALUE,
        Value::object(ns),
    );
    Ok(b)
}

/// GetModuleNamespace ( module ), 16.2.1.10, of a linked (or linking) module: made once.
/// The proxy is stored before its bindings are resolved, so that namespaces that
/// re-export each other (`export * as`) find each other.
pub(crate) fn namespace(heap: &mut Heap, m: Obj) -> Result<Obj, Fault> {
    if let Some(ns) = heap.object_slot(m, slot::NAMESPACE).as_object() {
        return Ok(ns);
    }
    let mods = modules(heap)?;
    let traps = heap
        .object_slot(mods, modules_slot::TRAPS)
        .as_object()
        .expect("namespace traps");
    let target = heap.alloc_object(ObjectClass::Ordinary, None, ns::COUNT)?;
    for i in 0..ns::COUNT {
        heap.set_object_slot(target, i, Value::UNDEFINED);
    }
    heap.set_object_slot(target, ns::MODULE, Value::object(m));
    let proxy = heap.new_proxy(target, traps, 0)?;
    heap.set_object_slot(m, slot::NAMESPACE, Value::object(proxy));
    // The exports that resolve, in code unit order (ambiguous ones are left out).
    let mut names = Vec::new();
    exported_names(heap, m, &mut Vec::new(), &mut names);
    let mut entries: Vec<(Vec<u16>, Atom, Resolution)> = Vec::new();
    for n in names {
        let r = resolve_export(heap, m, n, &mut Vec::new(), 0)?;
        if matches!(r, Resolution::Binding(..) | Resolution::Namespace(_)) {
            entries.push((heap.atom_name(n).encode_utf16().collect(), n, r));
        }
    }
    crate::sort::sort_by(&mut entries, |a, b| a.0 < b.0);
    let index = heap.new_object(None)?;
    let boxes = heap.alloc_value_array(entries.len() as u32)?;
    let keys = heap.alloc_u32_array(entries.len() as u32)?;
    heap.set_object_slot(target, ns::INDEX, Value::object(index));
    heap.set_object_slot(target, ns::BOXES, Value::internal(boxes));
    heap.set_object_slot(target, ns::KEYS, Value::internal(keys));
    for (i, &(_, n, r)) in entries.iter().enumerate() {
        let b = resolution_box(heap, r)?;
        heap.value_array_set(boxes, i as u32, b);
        heap.u32_array_set(keys, i as u32, n.id());
        realm::define_hidden(heap, index, n, Value::int(i as i32))?;
        let d = PropertyDescriptor::data(
            Value::UNDEFINED,
            PropFlags::WRITABLE | PropFlags::ENUMERABLE,
        );
        heap.define_own_property(target, n, &d)?;
    }
    heap.arena_mut().set(
        boxes,
        crate::heap::cells::ValueArray::LEN,
        entries.len() as u32,
    );
    let tag = heap.alloc_str("Module")?;
    let d = PropertyDescriptor::data(Value::string(tag), PropFlags::NONE);
    heap.define_own_property(target, atoms::SYM_TO_STRING_TAG, &d)?;
    heap.prevent_extensions(target);
    Ok(proxy)
}

/// The Box of export `key` of the namespace whose proxy target is `target`, if it has one.
fn namespace_binding(heap: &Heap, target: Obj, key: Atom) -> Option<Value> {
    let index = heap.object_slot(target, ns::INDEX).as_object()?;
    let i = heap.get_own_property(index, key)?.value()?.as_int()?;
    let boxes = heap.object_slot(target, ns::BOXES).as_internal()?;
    Some(heap.value_array_get(boxes, i as u32))
}

/// The value of a binding Box: ReferenceError in its TDZ.
fn binding_value(heap: &Heap, b: Value, key: Atom) -> Result<Value, Fault> {
    let v = box_value(heap, b);
    if v.is_uninitialized() {
        return Err(Fault::error(
            ErrorKind::ReferenceError,
            crate::text::cat!(heap.atom_name(key), " is not initialized"),
        ));
    }
    Ok(v)
}

/// `%NsOp`: internal method `op` of the namespace whose proxy target is `target`
/// (builtins/module.ts).
///
/// Ops 4 to 6 serve `import()` (module.ts ImportCall): 4 the async host function it calls
/// (or undefined), 5 the linked record of key `target` (an Error if there is none), 6 the
/// namespace of record `target`. 7 is IncrementModuleAsyncEvaluationCount, 8 a builtin
/// closure of `%ModuleOp` with data `target` and length `key` (module.ts's reactions).
pub(crate) fn namespace_op(
    heap: &mut Heap,
    op: i32,
    target: Value,
    key: Value,
    desc: Value,
) -> Result<Value, Fault> {
    let bad = || Fault::error(ErrorKind::InternalError, "bad %NsOp operation".into());
    match op {
        4 => return Ok(import_function(heap)),
        5 => {
            let k = target.as_string().ok_or_else(bad)?;
            let k = heap.str_to_string(k);
            return match find(heap, &k)? {
                Some(m) if status_of(heap, m) >= status::LINKED => Ok(Value::object(m)),
                _ => Err(Fault::error(
                    ErrorKind::Error,
                    crate::text::cat!("module '", &k, "' is not loaded"),
                )),
            };
        }
        6 => {
            return Ok(Value::object(namespace(
                heap,
                target.as_object().ok_or_else(bad)?,
            )?));
        }
        7 => {
            let m = modules(heap)?;
            let n = heap
                .object_slot(m, modules_slot::ASYNC_COUNT)
                .as_int()
                .unwrap_or(0)
                + 1;
            heap.set_object_slot(m, modules_slot::ASYNC_COUNT, Value::int(n));
            return Ok(Value::int(n));
        }
        8 => {
            let length = key.as_int().unwrap_or(0);
            return Ok(Value::object(module_op_closure(heap, target, length)?));
        }
        _ => {}
    }
    let target = target.as_object().ok_or_else(bad)?;
    match (op, desc.as_object()) {
        (0, _) => namespace_get(heap, target, key),
        (1, _) => namespace_descriptor(heap, target, key),
        (2, Some(desc)) => Ok(Value::bool(namespace_define(heap, target, key, desc)?)),
        (3, _) => namespace_keys(heap, target),
        _ => Err(bad()),
    }
}

/// A namespace's [[Get]] (10.4.6.8).
fn namespace_get(heap: &mut Heap, target: Obj, key: Value) -> Result<Value, Fault> {
    let key = crate::builtins::object::key(heap, key)?;
    if key.is_symbol() {
        return Ok(heap
            .get_own_property(target, key)
            .and_then(|p| p.value())
            .unwrap_or(Value::UNDEFINED));
    }
    match namespace_binding(heap, target, key) {
        Some(b) => binding_value(heap, b, key),
        None => Ok(Value::UNDEFINED),
    }
}

/// [[GetOwnProperty]] (10.4.6.5) as a descriptor object.
fn namespace_descriptor(heap: &mut Heap, target: Obj, key: Value) -> Result<Value, Fault> {
    let key = crate::builtins::object::key(heap, key)?;
    if key.is_symbol() {
        return crate::builtins::object::descriptor_object(heap, target, key);
    }
    let Some(b) = namespace_binding(heap, target, key) else {
        return Ok(Value::UNDEFINED);
    };
    let v = binding_value(heap, b, key)?;
    let proto = intrinsic_obj(heap, Intrinsic::ObjectProto);
    Ok(Value::object(heap.new_object_from_entries(
        Some(proto),
        &[
            atoms::VALUE,
            atoms::WRITABLE,
            atoms::ENUMERABLE,
            atoms::CONFIGURABLE,
        ],
        &[v, Value::TRUE, Value::TRUE, Value::FALSE],
    )?))
}

/// [[DefineOwnProperty]] (10.4.6.6) with a descriptor record.
fn namespace_define(heap: &mut Heap, target: Obj, key: Value, desc: Obj) -> Result<bool, Fault> {
    let key = crate::builtins::object::key(heap, key)?;
    if key.is_symbol() {
        return crate::builtins::object::define_from_record(heap, target, key, desc);
    }
    let Some(b) = namespace_binding(heap, target, key) else {
        return Ok(false);
    };
    let current = binding_value(heap, b, key)?;
    let field = |heap: &Heap, k: Atom| heap.get_own_property(desc, k).and_then(|p| p.value());
    let flag = |heap: &Heap, k: Atom| field(heap, k).map(|v| super::conv::to_boolean(heap, v));
    if flag(heap, atoms::CONFIGURABLE) == Some(true)
        || flag(heap, atoms::ENUMERABLE) == Some(false)
        || field(heap, atoms::GET).is_some()
        || field(heap, atoms::SET).is_some()
        || flag(heap, atoms::WRITABLE) == Some(false)
    {
        return Ok(false);
    }
    Ok(match field(heap, atoms::VALUE) {
        Some(v) => heap.same_value(v, current),
        None => true,
    })
}

/// [[OwnPropertyKeys]] (10.4.6.11) as a new array.
fn namespace_keys(heap: &mut Heap, target: Obj) -> Result<Value, Fault> {
    let keys = heap
        .object_slot(target, ns::KEYS)
        .as_internal()
        .expect("namespace keys");
    let n = heap.u32_array_len(keys);
    let mut values = Vec::with_capacity(n as usize + 1);
    for i in 0..n {
        let a = Atom::from_id(heap.u32_array_get(keys, i));
        values.push(heap.atom_to_value(a)?);
    }
    values.push(heap.atom_to_value(atoms::SYM_TO_STRING_TAG)?);
    let proto = intrinsic_obj(heap, Intrinsic::ArrayProto);
    Ok(Value::object(
        heap.new_array_from_values(Some(proto), &values)?,
    ))
}

/// The value of export `name` of linked module `m` (for the host): `None` if it has no
/// such export or the binding is uninitialized.
pub(crate) fn export_value(heap: &mut Heap, m: Obj, name: &str) -> Result<Option<Value>, Fault> {
    let name = heap.atom_str(name)?;
    let r = resolve_export(heap, m, name, &mut Vec::new(), 0)?;
    if !matches!(r, Resolution::Binding(..) | Resolution::Namespace(_)) {
        return Ok(None);
    }
    let b = resolution_box(heap, r)?;
    let v = box_value(heap, b);
    Ok((!v.is_uninitialized()).then_some(v))
}
