//! The base image's content (0127): what a shipped image needs and a VM that compiled
//! the prelude still has.

use alloc::vec;

use crate::bytecode::decode::insns;
use crate::bytecode::imm::ImmKind;
use crate::bytecode::template::{code, debug_info, func_flags};
use crate::heap::cells::{FuncTemplate, PropEntry, PropTable, Shape, Symbol, U32Array};
use crate::heap::{CellKind, Heap, HeapError, Ref};

/// Drops the prelude's internal names (D2253): the names of privileged functions
/// (`ArrayPrototypeMap`) and the atoms of its locals and parameters, which compiling the
/// prelude interned and nothing refers to. User code can't observe either: a builtin's
/// `name` is the installed property (D1433), and stack traces skip prelude frames
/// (D1980). The atoms that stay are the ones the heap refers to: property keys (property
/// tables, shape descriptors), symbols, function and local names of templates, the atom
/// immediates of the bytecode, and the global declarations' atom ids in constant pools
/// (int32 values: every int32 constant counts, a few false keeps at most).
///
/// The forgotten strings are garbage for the collection that follows.
pub(super) fn drop_internal_names(heap: &mut Heap) -> Result<u32, HeapError> {
    let templates: alloc::vec::Vec<u32> = heap
        .cells()
        .filter(|c| c.kind == CellKind::FuncTemplate)
        .map(|c| c.offset)
        .collect();
    for &t in &templates {
        let flags = u32::from(heap.arena().get::<u16>(t, FuncTemplate::FLAGS));
        if flags & (func_flags::PRIVILEGED | func_flags::LAZY_NAME) == func_flags::PRIVILEGED {
            heap.arena_mut().set(t, FuncTemplate::NAME, 0u32);
        }
    }
    // The names the engine interns at run time (D2067), so that using it interns none.
    let mut engine = vec![];
    for name in ENGINE_ATOMS {
        engine.push(heap.atom_str(name)?.id());
    }
    let mut keep = vec![false; heap.atom_count() as usize];
    let mut mark = |id: u32| {
        // String and symbol atoms (integer atoms have bit 31): their table index.
        if id & (1 << 31) == 0
            && let Some(k) = keep.get_mut((id & ((1 << 30) - 1)) as usize)
        {
            *k = true;
        }
    };
    for id in engine {
        mark(id);
    }
    let a = heap.arena();
    for c in heap.cells() {
        let at = c.offset;
        match c.kind {
            CellKind::Symbol => mark(a.get(at, Symbol::ATOM)),
            CellKind::PropTable => {
                for i in 0..a.get(at, PropTable::COUNT) {
                    mark(a.get(PropTable::ENTRIES.at(at, i), PropEntry::KEY));
                }
            }
            CellKind::Shape => {
                if let Some(d) = a.get::<Ref>(at, Shape::DESCS).get() {
                    for i in 0..a.read::<u32>(U32Array::DATA.at(d, 0)) {
                        mark(a.read(U32Array::DATA.at(d, 1 + 2 * i)));
                    }
                }
            }
            CellKind::FuncTemplate => {
                mark(a.get(at, FuncTemplate::NAME));
                if let Some(n) = debug_info(heap, at).names.get() {
                    for i in 0..a.get(n, U32Array::LEN) {
                        mark(a.read(U32Array::DATA.at(n, i)));
                    }
                }
                for insn in insns(code(heap, at)) {
                    let insn = insn.expect("the prelude's bytecode decodes");
                    for (k, kind) in insn.info().imms.iter().enumerate() {
                        if *kind == ImmKind::Atom {
                            mark(insn.imm(k));
                        }
                    }
                }
                if let Some(consts) = a.get::<Ref>(at, FuncTemplate::CONSTS).get() {
                    for i in 0..heap.value_array_len(consts) {
                        if let Some(n) = heap.value_array_get(consts, i).as_int() {
                            mark(n as u32);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let map = heap.forget_string_atoms(&keep)?;
    let forgotten = map.iter().filter(|&&m| m == u32::MAX).count() as u32;
    renumber_atoms(heap, &map)?;
    Ok(forgotten)
}

/// D2447: renumbers every atom id in the heap after `forget_string_atoms` closed the
/// holes of the forgotten ones (`map`: old table index to new): the heap's own structures
/// (`Heap::renumber_heap_atoms`), templates' names, local names, the atom immediates of
/// the code and the global declaration lists in constant pools.
fn renumber_atoms(heap: &mut Heap, map: &[u32]) -> Result<(), HeapError> {
    use crate::bytecode::Op;
    use crate::heap::Atom;
    use crate::heap::cells::FuncInfo;
    heap.renumber_heap_atoms(map)?;
    let moved = |a: u32| Heap::moved_atom(map, Atom::from_id(a)).id();
    let templates: alloc::vec::Vec<u32> = heap
        .cells()
        .filter(|c| c.kind == CellKind::FuncTemplate)
        .map(|c| c.offset)
        .collect();
    let mut decls = alloc::collections::BTreeSet::new();
    for &t in &templates {
        let name = heap.arena().get(t, FuncTemplate::NAME);
        heap.arena_mut().set(t, FuncTemplate::NAME, moved(name));
        if let Some(info) = heap.arena().get::<Ref>(t, FuncTemplate::INFO).get()
            && let Some(n) = heap.arena().get::<Ref>(info, FuncInfo::NAMES).get()
        {
            for i in 0..heap.arena().get(n, U32Array::LEN) {
                let at = U32Array::DATA.at(n, i);
                let id = heap.arena().read::<u32>(at);
                heap.arena_mut().write(at, moved(id));
            }
        }
        // The atom immediates (u32 wide), patched in place.
        let mut patches = vec![];
        for insn in insns(code(heap, t)) {
            let insn = insn.expect("the base's bytecode decodes");
            let mut at = insn.pc + 1;
            for (k, kind) in insn.info().imms.iter().enumerate() {
                if *kind == ImmKind::Atom {
                    patches.push((at, moved(insn.imm(k))));
                }
                at += u32::from(kind.width());
            }
            if insn.op == Op::GlobalDecls
                && let Some(p) = heap.arena().get::<Ref>(t, FuncTemplate::CONSTS).get()
            {
                decls.insert((p, insn.imm(0)));
            }
        }
        for (at, id) in patches {
            heap.arena_mut().write(FuncTemplate::DATA.at(t, at), id);
        }
    }
    // `n_var, n_func, n_let, n_const`, then the atom ids (ops.rs `global_decls`).
    for (p, idx) in decls {
        let count = |k: u32| heap.value_array_get(p, idx + k).as_int().unwrap_or(0) as u32;
        let n: u32 = (0..4).map(count).sum();
        for i in idx + 4..idx + 4 + n {
            let id = heap.value_array_get(p, i).as_int().unwrap_or(0) as u32;
            heap.value_array_set(p, i, crate::value::Value::int(moved(id) as i32));
        }
    }
    Ok(())
}

/// D2446: drops the entries of the shared constant pools that no live template uses (a
/// unit's pool also holds the constants of its functions that died, the prelude's
/// installing code), keeping their order: every index only gets smaller, so each
/// `push_const8` still fits and the code is patched in place. Runs after a collection.
pub(super) fn prune_pools(heap: &mut Heap) -> Result<(), HeapError> {
    use crate::bytecode::Op;
    use crate::heap::cells::ValueArray;
    use alloc::collections::BTreeMap;
    let mut by_pool: BTreeMap<u32, alloc::vec::Vec<u32>> = BTreeMap::new();
    for c in heap.cells().filter(|c| c.kind == CellKind::FuncTemplate) {
        if let Some(p) = heap
            .arena()
            .get::<Ref>(c.offset, FuncTemplate::CONSTS)
            .get()
        {
            by_pool.entry(p).or_default().push(c.offset);
        }
    }
    for (pool, templates) in by_pool {
        let len = heap.value_array_len(pool);
        let get = |heap: &Heap, i: u32| heap.value_array_get(pool, i);
        let mut used = vec![false; len as usize];
        // Tagged template sites still holding the start of their strings (an int32;
        // after the first evaluation they hold the template object).
        let mut sites = vec![];
        let mark = |used: &mut [bool], from: u32, n: u32| {
            for u in &mut used[from as usize..(from + n) as usize] {
                *u = true;
            }
        };
        for &t in &templates {
            for insn in insns(code(heap, t)) {
                let insn = insn.expect("the base's bytecode decodes");
                let i = insn.imm(0);
                match insn.op {
                    Op::PushConst | Op::PushConst8 => mark(&mut used, i, 1),
                    Op::GetTemplateObject => {
                        mark(&mut used, i, 1);
                        if let Some(start) = get(heap, i).as_int() {
                            let n = get(heap, start as u32).as_int().unwrap_or(0) as u32;
                            mark(&mut used, start as u32, 1 + 2 * n);
                            sites.push(i);
                        }
                    }
                    Op::GlobalDecls => {
                        let counts: u32 = (0..4)
                            .map(|k| get(heap, i + k).as_int().unwrap_or(0) as u32)
                            .sum();
                        mark(&mut used, i, 4 + counts);
                    }
                    _ => {}
                }
            }
        }
        let kept = used.iter().filter(|&&u| u).count() as u32;
        if kept == len {
            continue;
        }
        let mut map = vec![u32::MAX; len as usize];
        let mut next = 0;
        for (i, &u) in used.iter().enumerate() {
            if u {
                map[i] = next;
                next += 1;
            }
        }
        let new = heap.alloc_value_array(kept)?;
        heap.arena_mut().set(new, ValueArray::LEN, kept);
        for i in 0..len {
            if used[i as usize] {
                let mut v = get(heap, i);
                if sites.contains(&i) {
                    v = crate::value::Value::int(map[v.as_int().unwrap_or(0) as usize] as i32);
                }
                heap.value_array_set(new, map[i as usize], v);
            }
        }
        for &t in &templates {
            let patches: alloc::vec::Vec<(u32, Op, u32)> = insns(code(heap, t))
                .filter_map(|insn| {
                    let insn = insn.ok()?;
                    matches!(
                        insn.op,
                        Op::PushConst | Op::PushConst8 | Op::GetTemplateObject | Op::GlobalDecls
                    )
                    .then(|| (insn.pc, insn.op, map[insn.imm(0) as usize]))
                })
                .collect();
            let a = heap.arena_mut();
            for (pc, op, idx) in patches {
                let at = FuncTemplate::DATA.at(t, pc + 1);
                if op == Op::PushConst8 {
                    a.write(at, idx as u8);
                } else {
                    a.write(at, idx);
                }
            }
            a.set(t, FuncTemplate::CONSTS, Ref(new));
        }
    }
    Ok(())
}

/// D2445: the builtin closures whose `length` and `name` are the ones `%Install*`
/// defined get them lazily from their template (`Heap::make_builtin_props_lazy`), if no
/// other closure shares it. Before the base's shape tree is replaced: the rebuilt
/// shapes are the prelude's. Returns how many.
pub(super) fn lazy_builtin_props(heap: &mut Heap) -> Result<u32, HeapError> {
    use crate::heap::ObjectClass;
    use alloc::collections::BTreeMap;
    let mut closures: alloc::vec::Vec<(crate::value::Obj, u32)> = vec![];
    let mut per_template: BTreeMap<u32, u32> = BTreeMap::new();
    for c in heap.cells().filter(|c| c.kind == CellKind::Object) {
        let o = crate::value::Obj(c.offset);
        if heap.object_class(o) != ObjectClass::Function {
            continue;
        }
        let t = heap.closure_template(o);
        closures.push((o, t));
        *per_template.entry(t).or_default() += 1;
    }
    let mut n = 0;
    for (o, t) in closures {
        if per_template[&t] == 1
            && u32::from(heap.arena().get::<u16>(t, FuncTemplate::FLAGS)) & func_flags::PRIVILEGED
                != 0
            && heap.make_builtin_props_lazy(o)?
        {
            n += 1;
        }
    }
    Ok(n)
}

/// The shapes of the objects that builtins and the engine create with named properties,
/// as key chains with their stored flags (D2067). The base has them in every VM's shape
/// tree, so using a builtin adds no shape, whatever the order. From a survey of builtin
/// calls (0148): iterator results; RegExp match results and their `indices`; property
/// descriptors; functions, classes and their prototypes; RegExp objects; errors;
/// `Promise.allSettled`, `Promise.withResolvers` and `Proxy.revocable` results; template
/// objects.
pub(super) const BUILTIN_RESULT_SHAPES: &[&[(&str, u32)]] = {
    const W: u32 = 1;
    const ALL: u32 = 7;
    const WC: u32 = 5;
    const C: u32 = 4;
    &[
        &[("value", ALL), ("done", ALL)],
        &[
            ("index", ALL),
            ("input", ALL),
            ("groups", ALL),
            ("indices", ALL),
        ],
        &[("groups", ALL)],
        &[
            ("value", ALL),
            ("writable", ALL),
            ("enumerable", ALL),
            ("configurable", ALL),
        ],
        &[
            ("get", ALL),
            ("set", ALL),
            ("enumerable", ALL),
            ("configurable", ALL),
        ],
        // Builtin and bound closures; a closure's own `length` and `name` are lazy
        // (D2445), so its `prototype` is its first stored key.
        &[("length", C), ("name", C)],
        &[("prototype", W)],
        &[("prototype", 0)],
        &[("constructor", WC)],
        &[("lastIndex", W)],
        &[("message", WC), ("cause", WC)],
        &[("message", WC), ("errors", WC)],
        &[("message", WC), ("cause", WC), ("errors", WC)],
        &[("cause", WC)],
        &[("errors", WC)],
        &[("status", ALL), ("value", ALL)],
        &[("status", ALL), ("reason", ALL)],
        &[("promise", ALL), ("resolve", ALL), ("reject", ALL)],
        &[("proxy", ALL), ("revoke", ALL)],
        &[("raw", 0)],
    ]
};

/// Room for a VM's own atoms in the base image's tail (D2431): a session adding up to
/// this many writes them in place (a fresh REPL adds about 16).
pub(super) const SESSION_ATOMS: u32 = 32;

/// Names the engine interns while it runs, whatever the script (D2067): the realm's
/// `Error` hooks, read when an error is made. (The names every compilation interns are
/// interned by a session's first compilation, before anything else it does.)
pub(super) const ENGINE_ATOMS: &[&str] = &["prepareStackTrace", "stackTraceLimit"];
