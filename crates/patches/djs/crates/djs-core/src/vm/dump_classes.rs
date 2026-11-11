//! Classes as the exporter reads them (D2580, D2878): whether a function is a class
//! constructor, and the private names its code can use.

use alloc::string::String;
use alloc::vec::Vec;

use crate::heap::cells::{BoxCell, FuncTemplate};
use crate::heap::{Heap, ObjectClass, Property};
use crate::value::{Obj, Sym, Value};

/// Whether `f` is a class constructor.
pub(super) fn is_class(h: &Heap, f: Obj) -> bool {
    h.object_class(f) == ObjectClass::Function && {
        let t = h.closure_template(f);
        let kind = crate::bytecode::template::func_flags::kind(u32::from(
            h.arena().get::<u16>(t, FuncTemplate::FLAGS),
        ));
        matches!(
            kind,
            crate::bytecode::template::func_flags::KIND_CLASS_CTOR
                | crate::bytecode::template::func_flags::KIND_DERIVED_CTOR
        )
    }
}

/// A private name of a class: its symbol, and for a private method or accessor the
/// closures the class installs on its instances.
pub(super) struct ClassPrivate {
    pub name: String,
    pub sym: Sym,
    pub method: Value,
    pub getter: Value,
    pub setter: Value,
    /// Two different symbols of this name were found (a nested class's).
    pub ambiguous: bool,
}

/// The private names of class `c` that its code can use: the variables named `#x` that
/// its constructor, its methods and accessors, and the hidden closures they hold (field
/// initializers, private methods) capture. A private name no closure of the class
/// captures can no longer be read by anyone.
pub(super) fn class_privates(h: &Heap, c: Obj) -> Vec<ClassPrivate> {
    let mut out: Vec<ClassPrivate> = Vec::new();
    let mut queue: Vec<Obj> = alloc::vec![c];
    let mut seen: Vec<u32> = alloc::vec![c.offset()];
    let push = |queue: &mut Vec<Obj>, seen: &mut Vec<u32>, v: Value| {
        if let Some(f) = v.as_object()
            && h.object_class(f) == ObjectClass::Function
            && (f == c || !is_class(h, f))
            && !seen.contains(&f.offset())
        {
            seen.push(f.offset());
            queue.push(f);
        }
    };
    // Members: the class's own and its prototype's methods and accessors.
    let mut holders = alloc::vec![c];
    if let Some(Property::Data { value, .. }) = h.get_own_property(c, crate::heap::atoms::PROTOTYPE)
        && let Some(p) = value.as_object()
    {
        holders.push(p);
    }
    for o in holders {
        for k in h.own_keys(o) {
            match h.get_own_property(o, k) {
                Some(Property::Data { value, .. }) => push(&mut queue, &mut seen, value),
                Some(Property::Accessor { getter, setter, .. }) => {
                    push(&mut queue, &mut seen, getter);
                    push(&mut queue, &mut seen, setter);
                }
                None => {}
            }
        }
    }
    let mut i = 0;
    while i < queue.len() {
        let f = queue[i];
        i += 1;
        let t = h.closure_template(f);
        let a = h.arena();
        let locals = usize::from(a.get::<u16>(t, FuncTemplate::LOCALS));
        let captures = u32::from(a.get::<u16>(t, FuncTemplate::CAPTURES));
        let names = super::dump::template_names(h, t);
        for j in 0..captures {
            let Some(name) = names.get(locals + j as usize) else {
                continue;
            };
            let Some(b) = h.object_slot(f, j).as_internal() else {
                continue;
            };
            let v: Value = a.get(b, BoxCell::VALUE);
            if name.starts_with('#') {
                let Some(sym) = v.as_symbol() else { continue };
                match out.iter_mut().find(|p| p.name == *name) {
                    Some(p) if p.sym.0 == 0 => p.sym = sym,
                    Some(p) => p.ambiguous |= p.sym != sym,
                    None => out.push(ClassPrivate {
                        name: name.clone(),
                        sym,
                        method: Value::UNDEFINED,
                        getter: Value::UNDEFINED,
                        setter: Value::UNDEFINED,
                        ambiguous: false,
                    }),
                }
            } else if let Some(inner) = name.strip_prefix('<').and_then(|n| n.strip_suffix('>')) {
                // `<#m>`, `<get #x>`, `<set #x>`: a private method's closures; `<fields
                // N>` and others: hidden closures of the class.
                let (slot, pname) = match inner.split_once(' ') {
                    Some(("get", n)) if n.starts_with('#') => (1, n),
                    Some(("set", n)) if n.starts_with('#') => (2, n),
                    _ if inner.starts_with('#') => (0, inner),
                    _ => (3, ""),
                };
                if slot < 3 {
                    let p = match out.iter_mut().position(|p| p.name == pname) {
                        Some(p) => p,
                        None => {
                            // The method's name, before its symbol is seen.
                            out.push(ClassPrivate {
                                name: String::from(pname),
                                sym: Sym(0),
                                method: Value::UNDEFINED,
                                getter: Value::UNDEFINED,
                                setter: Value::UNDEFINED,
                                ambiguous: false,
                            });
                            out.len() - 1
                        }
                    };
                    match slot {
                        0 => out[p].method = v,
                        1 => out[p].getter = v,
                        _ => out[p].setter = v,
                    }
                }
                push(&mut queue, &mut seen, v);
            }
        }
    }
    out.retain(|p| p.sym.0 != 0);
    out
}
