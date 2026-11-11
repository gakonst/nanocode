//! The plain dump runtime (D2834, D2903): the format's fixed helpers, which every host
//! provides as members of `$djs` next to its hooks (`seed`, `resume`, `source`, `host`), so that
//! a dump does not carry them. They are the same code in every dump: a small standard
//! library of the format, not program content.
//!
//! [`HELPERS`] is the one definition. Hosts on djs get the object from
//! [`Vm::dump_runtime`] (the CLI, the difftest) or, with the text from
//! js/src/dump-runtime.js, [`Vm::dump_runtime_from`] (the wasm export `vm_dump_runtime` for
//! the Sandbox and pi's adapter, so that the shipped module does not carry the text); hosts
//! without djs (node) import js/src/dump-runtime.js, which [`module_text`] writes from the
//! same table (a test keeps the file current).
//!
//! Every helper is a plain function (or a builtin) that depends neither on `this` nor on
//! another member through the object (`FM` and `CC` call `L` lexically): `$djs` is a global for now and meant to become
//! imports, one named export per helper.

use alloc::string::String;
use alloc::vec::Vec;

use super::Vm;
use crate::interp::run::Status;
use crate::value::Value;

/// What the helpers share lexically (not members): the builtins they call, taken when the
/// runtime is evaluated, before any user code.
const SHARED: &str = "const matchAll = RegExp.prototype[Symbol.matchAll];";

/// The helpers, `(name, expression)`, in the order the runtime defines them.
pub(crate) const HELPERS: &[(&str, &str)] = &[
    ("R", "Reflect.apply"),
    ("G", "Reflect.get"),
    ("C", "Reflect.construct"),
    (
        "D",
        "(o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, enumerable: true, configurable: true })",
    ),
    ("A", "Object.assign"),
    // An iterable over a saved iterator record whose first step is a placeholder: the
    // rest of the current iteration runs in it, the next ones take the iterator's steps.
    // `set(it, next)` gives it the record later, where the dump's callback makes the
    // iterator inside the loop's first iteration (D2910).
    (
        "L",
        "(it, next) => { let first = true; const r = { next() { if (first) { first = false; return { value: undefined, done: false }; } return Reflect.apply(next ?? it.next, it, []); }, get return() { const m = it.return; return m == null ? m : (...v) => Reflect.apply(m, it, v); }, set(i, n) { it = i; next = n; }, [Symbol.iterator]() { return r; } }; return r; }",
    ),
    // The same without the placeholder (a pattern's element whose step is to come, a loop
    // going on after the rest of its iteration; `next` left out: the iterator's own), and
    // an iterable that is done (a pattern's iterator that was).
    (
        "I",
        "(it, next) => { const r = { next() { return Reflect.apply(next ?? it.next, it, []); }, get return() { const m = it.return; return m == null ? m : (...v) => Reflect.apply(m, it, v); }, [Symbol.iterator]() { return r; } }; return r; }",
    ),
    (
        "E",
        "() => ({ [Symbol.iterator]() { return { next() { return { value: undefined, done: true }; } }; } })",
    ),
    // A `yield*`'s inner record (D2875): the first step is `first` if given, else a
    // placeholder (the delegation's yield the original is suspended at); `throw` and
    // `return` are the inner iterator's, looked up when the delegation uses them. With
    // `a` 2 (async, awaiting the inner `return()`, D2887) the placeholder comes first and
    // the first `return` gives `first`.
    (
        "Y",
        "(it, next, a, ...first) => { const r2 = a > 1; let ph = r2 || !first.length; const m = (k) => { if (r2 && k == 'return' && first.length) { const p = first.pop(); return () => p; } const f = it[k]; return f == null ? f : (...v) => Reflect.apply(f, it, v); }; const r = { next(...v) { if (!r2 && first.length) return first.pop(); if (ph) { ph = false; return { value: undefined, done: false }; } return Reflect.apply(next, it, v); }, get throw() { return m('throw'); }, get return() { return m('return'); }, [a ? Symbol.asyncIterator : Symbol.iterator]() { return r; } }; return r; }",
    ),
    // A `using` resource added again (D2876): its saved dispose method called on its
    // value; the exporter reads `$djs_v`/`$djs_m` back.
    // An object literal's method, getter or setter after a continuation's site (D2880):
    // defined on the object with its descriptor from a literal of its own. A getter's
    // descriptor has no `set` (nor a setter's a `get`), as the literal's definition: a
    // getter and a setter of one key make one accessor (0207).
    (
        "DD",
        "(o, src) => { for (const k of Reflect.ownKeys(src)) { const d = Reflect.getOwnPropertyDescriptor(src, k); if ('get' in d) { if (d.get === undefined) delete d.get; if (d.set === undefined) delete d.set; } Object.defineProperty(o, k, d); } return o; }",
    ),
    (
        "U",
        "($djs_v, $djs_m, k) => k === 4 ? { [Symbol.asyncDispose]: () => Reflect.apply($djs_m, $djs_v, []) } : { [Symbol.dispose]: () => Reflect.apply($djs_m, $djs_v, []) }",
    ),
    // Iterator helpers made again (D2877): a callback whose counter is shifted, a
    // flatMap inside its inner iterator and a concat inside an item (made one step
    // before, stepped over a placeholder), a completed helper, a concat item. (`L` is
    // the runtime's own, lexically.)
    (
        "M",
        "($djs_f, $djs_k) => (x, i) => Reflect.apply($djs_f, undefined, [x, i + $djs_k])",
    ),
    (
        "FM",
        "(it, next, $djs_f, k, inner, innerNext) => { const $djs_k = k - 1, $djs_in = { [Symbol.iterator]: () => L(inner, innerNext) }; const h = Iterator.prototype.flatMap.call(L(it, next), (x, i) => i === 0 ? $djs_in : Reflect.apply($djs_f, undefined, [x, i + $djs_k])); h.next(); return h; }",
    ),
    (
        "CC",
        "(cur, curNext, items) => { const h = Iterator.concat({ [Symbol.iterator]: () => L(cur, curNext) }, ...items); h.next(); return h; }",
    ),
    (
        "CI",
        "($djs_m, $djs_o) => ({ [Symbol.iterator]: () => Reflect.apply($djs_m, $djs_o, []) })",
    ),
    (
        "HD",
        "() => { const h = Iterator.prototype.map.call({ next: () => ({ value: undefined, done: true }) }, (x) => x); h.next(); return h; }",
    ),
    // ToPropertyKey, as a computed key does it.
    ("P", "(k) => Reflect.ownKeys({ [k]: 0 })[0]"),
    // A pattern's rest property: CopyDataProperties without the keys taken.
    (
        "S",
        "(o, ex) => { const r = {}, f = Object(o), ks = Reflect.ownKeys(f); for (let i = 0; i < ks.length; i++) { const k = ks[i]; let skip = false; for (let j = 0; j < ex.length; j++) if (ex[j] === k) skip = true; if (skip) continue; const d = Reflect.getOwnPropertyDescriptor(f, k); if (d && d.enumerable) Object.defineProperty(r, k, { value: f[k], writable: true, enumerable: true, configurable: true }); } return r; }",
    ),
    // An async iterator over a saved record for a `for await`: its first step `first`
    // (the promise the loop was awaiting) if given. `return` is the iterator's own,
    // looked up when the loop closes, so a missing one is not called.
    (
        "N",
        "(it, next, ...first) => { const r = { next() { return first.length ? first.pop() : Reflect.apply(next, it, []); }, get return() { const m = it.return; return m == null ? m : (...v) => Reflect.apply(m, it, v); }, [Symbol.asyncIterator]() { return r; } }; return r; }",
    ),
    // AsyncIteratorClose of an exception: a promise to await that always fulfills.
    (
        "X",
        "(it) => { let p; try { const m = it.return; if (m == null) return; p = Promise.resolve(Reflect.apply(m, it, [])); } catch { return; } return p.then(() => {}, () => {}); }",
    ),
    // The check of an iterator result.
    (
        "O",
        "(r) => { if (Object(r) !== r) throw new TypeError('iterator must return an object'); }",
    ),
    // The async-from-sync iterator a `for await` over a sync iterable makes (never
    // visible to user code): its `next` and `return` step as the language's do.
    (
        "fromSync",
        "(it, next) => { const q = () => { try { const m = it.return; if (m != null) Reflect.apply(m, it, []); } catch {} }; const step = (f, close) => new Promise((res, rej) => { const r = f(); if (Object(r) !== r) throw new TypeError('iterator must return an object'); const done = !!r.done; let w; try { w = Promise.resolve(r.value); } catch (e) { if (!done && close) q(); throw e; } w.then((v) => res({ value: v, done }), !done && close ? (e) => { q(); rej(e); } : rej); }); return { next: (...a) => step(() => Reflect.apply(next, it, a), true), return: (...a) => { const m = it.return; return m == null ? Promise.resolve({ value: a[0], done: true }) : step(() => Reflect.apply(m, it, a), false); }, throw: (...a) => { const m = it.throw; if (m != null) return step(() => Reflect.apply(m, it, a), true); try { const c = it.return; if (c != null) { const r = Reflect.apply(c, it, []); if (Object(r) !== r) throw new TypeError('iterator must return an object'); } throw new TypeError('throw is not a method'); } catch (e) { return Promise.reject(e); } } }; }",
    ),
    (
        "Q",
        "(a, ...v) => { let n = a.length; for (const x of v) Object.defineProperty(a, n++, { value: x, writable: true, enumerable: true, configurable: true }); return a; }",
    ),
    (
        "regexpIterator",
        "(re, s, f) => { const it = Reflect.apply(matchAll, re, [s]); if (f & 4) while (!it.next().done); return it; }",
    ),
    // The async-from-sync wrapper's handler that closes the sync iterator when a step's
    // value rejects (IteratorClose of a throw: its errors are ignored), and rethrows.
    (
        "closeThrow",
        "(it) => (e) => { try { const m = it.return; if (m != null) Reflect.apply(m, it, []); } catch {} throw e; }",
    ),
    (
        "forIn",
        "(o, keys) => { let i = 0; const r = { next() { while (i < keys.length) { const k = keys[i++]; if (o == null || k in o) return { value: k, done: false }; } return { value: undefined, done: true }; }, [Symbol.iterator]() { return r; } }; return r; }",
    ),
    (
        "advance",
        "(it, n) => { const next = it.next; for (let i = 0; i < n; i++) Reflect.apply(next, it, []); return it; }",
    ),
    (
        "fn",
        "(o) => { const d = Object.getOwnPropertyDescriptor(o, Reflect.ownKeys(o)[0]); return 'value' in d ? d.value : d.get ?? d.set; }",
    ),
    (
        "spent",
        "(k) => { let f; new Promise((a, b) => { f = k ? b : a; a(); }); return f; }",
    ),
    (
        "rejected",
        "(e) => { const p = Promise.reject(e); p.then(undefined, () => {}); return p; }",
    ),
    // Array.fromAsync waiting on an await (0215): its closure again over its state, from
    // the phase it waits in (builtins/fromasync.ts); its promise is the one returned.
    (
        "fromAsync",
        "(p, mapfn, thisArg, it, next, A, k, phase, items, len, error) => new Promise((resolve, reject) => { const put = (v) => { if (!Reflect.defineProperty(A, k, { value: v, writable: true, enumerable: true, configurable: true })) throw new TypeError('cannot define the element'); }; const wait = (v, ph) => { phase = ph; Promise.prototype.then.call(Promise.resolve(v), ok, bad); }; const close = (e) => { error = e; try { const m = it.return; if (m == null) { reject(e); return; } wait(Reflect.apply(m, it, []), 3); } catch { reject(error); } }; const step = () => { try { if (k >= 9007199254740991) { close(new TypeError('Array too long')); return; } wait(Reflect.apply(next, it, []), 1); } catch (e) { reject(e); } }; const store = (v) => { try { put(v); } catch (e) { close(e); return; } k++; step(); }; const result = (r) => { let v; try { if (Object(r) !== r) throw new TypeError('iterator must return an object'); if (r.done) { A.length = k; resolve(A); return; } v = r.value; } catch (e) { reject(e); return; } if (mapfn === undefined) { store(v); return; } try { wait(Reflect.apply(mapfn, thisArg, [v, k]), 2); } catch (e) { close(e); } }; const element = () => { try { if (k < len) { wait(items[k], 4); return; } A.length = len; resolve(A); } catch (e) { reject(e); } }; const value = (v) => { try { if (phase === 4 && mapfn !== undefined) { wait(Reflect.apply(mapfn, thisArg, [v, k]), 5); return; } put(v); k++; } catch (e) { reject(e); return; } element(); }; const ok = (v) => { if (phase === 1) result(v); else if (phase === 2) store(v); else if (phase === 3) reject(error); else value(v); }; const bad = (e) => { if (phase === 2) close(e); else if (phase === 3) reject(error); else reject(e); }; Promise.prototype.then.call(p, ok, bad); })",
    ),
    ("bytes", "(b) => Uint8Array.fromBase64(b).buffer"),
    // A continuation's origin record (D2833, D2913: its origin, site, tape and holes) and a
    // generator continuation's prototype, its origin's where not given (D2915), given
    // where the dump writes the continuation (D2909); returns it.
    (
        "k",
        "(K, f, at, r, h, p) => { Object.defineProperty(K, '$djs', { value: [f, at, r, h] }); if (p !== undefined) K.prototype = p; else if ('prototype' in K) K.prototype = f.prototype; return K; }",
    ),
    (
        "keep",
        "(o, keys) => { for (const k of Reflect.ownKeys(o)) if (!['length', 'name', 'prototype', 'constructor'].includes(k) && !keys.includes(k) && !delete o[k]) throw new TypeError('keep'); }",
    ),
];

/// The runtime as a Script whose completion value is the helpers object.
pub fn script_text() -> String {
    script_of(&[true; HELPERS.len()])
}

/// [`script_text`] with only the helpers `dump` names (`$djs.NAME`) and the ones their
/// texts name: a load compiles a few helpers instead of all of them (0215). Generous: a
/// name in a string or a parameter that is also a helper's adds that helper.
pub fn script_text_for(dump: &str) -> String {
    let mut want = [false; HELPERS.len()];
    let mut todo: Vec<usize> = Vec::new();
    let add = |name: &str, want: &mut [bool], todo: &mut Vec<usize>| {
        if let Some(i) = HELPERS.iter().position(|h| h.0 == name)
            && !want[i]
        {
            want[i] = true;
            todo.push(i);
        }
    };
    let mut rest = dump;
    while let Some(p) = rest.find("$djs.") {
        rest = &rest[p + 5..];
        let n = rest
            .bytes()
            .position(|b| !(b.is_ascii_alphanumeric() || b == b'_' || b == b'$'))
            .unwrap_or(rest.len());
        add(&rest[..n], &mut want, &mut todo);
    }
    while let Some(i) = todo.pop() {
        for w in HELPERS[i]
            .1
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        {
            add(w, &mut want, &mut todo);
        }
    }
    script_of(&want)
}

fn script_of(want: &[bool]) -> String {
    let mut s = String::from("\"use strict\";\n(() => {\n");
    s.push_str(SHARED);
    s.push('\n');
    for (i, (k, v)) in HELPERS.iter().enumerate() {
        if want[i] {
            s.push_str("const ");
            s.push_str(k);
            s.push_str(" = ");
            s.push_str(v);
            s.push_str(";\n");
        }
    }
    s.push_str("return { ");
    let mut first = true;
    for (i, (k, _)) in HELPERS.iter().enumerate() {
        if want[i] {
            if !first {
                s.push_str(", ");
            }
            first = false;
            s.push_str(k);
        }
    }
    s.push_str(" };\n})();\n");
    s
}

/// The runtime as an ES module for hosts without djs (js/src/dump-runtime.js): one named
/// export per helper and `helpers`, all of them by name.
pub fn module_text() -> String {
    let mut s = String::from(
        "// The plain dump runtime (D2834, D2903): the helpers a plain dump calls as `$djs.NAME`.\n\
         // To run a dump outside djs, set `globalThis.$djs` to these and the host's hooks\n\
         // (`seed`, `resume`, `source`, `host`; see docs/plain-js-dumps.md, \"The host interface\"), then run the\n\
         // dump as a Script, and delete `$djs` and `$dump` after it. Generated from crates/djs-core/src/vm/dump_runtime.rs:\n\
         //   DJS_UPDATE_DUMP_RUNTIME=1 cargo test -p djs-core dump_runtime\n\n",
    );
    s.push_str(SHARED);
    s.push('\n');
    for (k, v) in HELPERS {
        s.push_str("export const ");
        s.push_str(k);
        s.push_str(" = ");
        s.push_str(v);
        s.push_str(";\n");
    }
    s.push_str("\n/** The helpers by name: `globalThis.$djs = { ...helpers, seed, resume }`. */\nexport const helpers = { ");
    for (i, (k, _)) in HELPERS.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(k);
    }
    s.push_str(" };\n");
    // The text has no `\`, backquote or `${`: a template literal holds it as is.
    s.push_str(
        "\n/** The same as a Script whose completion value is `helpers`, for a host that evaluates it in\n * a djs VM itself (`vm_dump_runtime`). */\nexport const script = `",
    );
    s.push_str(&script_text());
    s.push_str("`;\n");
    s.push_str(SCRIPT_FOR);
    s
}

/// [`script_text_for`] in JavaScript, over `script`'s lines (one helper each).
const SCRIPT_FOR: &str = r#"
/** `script` with only the helpers `dump` names (`$djs.NAME`) and the ones their texts name:
 * a load compiles a few helpers instead of all of them. */
export function scriptFor(dump) {
  const lines = script.split("\n");
  const def = (l) => (l.startsWith("const ") && !l.startsWith("const matchAll ") ? l.slice(6, l.indexOf(" = ")) : undefined);
  const defs = new Map(lines.filter(def).map((l) => [def(l), l]));
  const want = new Set();
  const add = (n) => {
    if (!defs.has(n) || want.has(n)) return;
    want.add(n);
    for (const w of defs.get(n).slice(n.length + 9).split(/[^\w$]+/)) add(w);
  };
  for (const m of dump.split("$djs.").slice(1)) add(/^[\w$]*/.exec(m)[0]);
  const names = [...defs.keys()].filter((n) => want.has(n));
  return lines
    .filter((l) => def(l) === undefined || want.has(def(l)))
    .map((l) => (l.startsWith("return { ") ? `return { ${names.join(", ")} };` : l))
    .join("\n");
}
"#;

impl Vm {
    /// Ends a plain dump's load, once its Script was evaluated (D2916): deletes the
    /// globals the load used, the host's `$djs` and the dump's `$dump`. A dump of the VM
    /// refuses either name.
    pub fn end_dump_load(&mut self) -> Result<(), crate::heap::HeapError> {
        let g = self.global();
        for name in ["$djs", "$dump"] {
            let k = self.heap.atom_str(name)?;
            self.heap.delete_own_property(g, k)?;
        }
        Ok(())
    }

    /// A new object holding the plain dump runtime's helpers (D2903), for the host to add
    /// its hooks to and define as the global `$djs` while a plain dump loads. Evaluates
    /// the runtime: call it while the VM is not running (before loading the dump).
    pub fn dump_runtime(&mut self) -> Result<Value, String> {
        self.dump_runtime_from(&script_text())
    }

    /// [`Vm::dump_runtime`] with only the helpers dump `text` names (0215): what a host
    /// that has the dump calls.
    pub fn dump_runtime_for(&mut self, text: &str) -> Result<Value, String> {
        self.dump_runtime_from(&script_text_for(text))
    }

    /// [`Vm::dump_runtime`] from the runtime's [`script_text`] the host passes: the wasm
    /// module's host has it (js/src/dump-runtime.js `script`), so the module does not
    /// carry it.
    pub fn dump_runtime_from(&mut self, script: &str) -> Result<Value, String> {
        // (Plain messages: formatting the errors' Debug would pull their code into the
        // shipped module, and the runtime is fixed text that fails only without memory.)
        // Eager even in a lazy VM (D2995): a host passes the helpers its dump names, and
        // the load calls them.
        let fail = |_| String::from("the dump runtime does not compile");
        let opts = crate::compiler::CompileOptions::script("djs-dump-runtime.js");
        let t = crate::compiler::compile(&mut self.heap, script.as_bytes(), &opts)
            .map_err(|_| String::from("the dump runtime does not compile"))?;
        let f = self.instantiate(t).map_err(fail)?;
        let this = Value::object(self.global());
        self.call(f, this, &[])
            .map_err(|_| String::from("the dump runtime: the VM is busy"))?;
        match self.run(u64::MAX) {
            Status::Done(v) => Ok(v),
            // Async host calls of an earlier run are pending: the runtime's own call is
            // done, its value is the entry result.
            Status::Idle => Ok(crate::interp::state::result(&self.heap)),
            _ => Err(String::from("the dump runtime did not finish")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// js/src/dump-runtime.js must be [`module_text`]. With `DJS_UPDATE_DUMP_RUNTIME=1`
    /// the test rewrites it.
    #[test]
    fn dump_runtime_js_is_current() {
        extern crate std;
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../js/src/dump-runtime.js");
        let script = script_text();
        assert!(!script.contains('\\') && !script.contains('`') && !script.contains("${"));
        let want = module_text();
        if std::env::var("DJS_UPDATE_DUMP_RUNTIME").is_ok() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "js/src/dump-runtime.js is out of date: run DJS_UPDATE_DUMP_RUNTIME=1 cargo test -p djs-core dump_runtime"
        );
    }

    /// A dump's subset holds the helpers it names and those they call (`FM` calls `L`),
    /// and evaluates.
    #[test]
    fn the_subset_for_a_dump() {
        let s = script_text_for("const $djs_FM = $djs.FM; $djs.k(f, {});");
        for want in [
            "const FM = ",
            "const L = ",
            "const k = ",
            "return { L, FM, k };",
        ] {
            assert!(s.contains(want), "{want}: {s}");
        }
        assert!(!s.contains("const Y = "), "{s}");
        let mut vm = Vm::new(Default::default()).unwrap();
        let o = vm.dump_runtime_for("$djs.FM").unwrap().as_object().unwrap();
        // (FM, L, and k: a parameter of FM shares its name.)
        assert_eq!(vm.heap().own_keys(o).len(), 3);
        assert_eq!(script_text_for(""), script_of(&[false; HELPERS.len()]));
    }

    /// The runtime evaluates in a VM and holds every helper.
    #[test]
    fn the_runtime_evaluates() {
        let mut vm = Vm::new(Default::default()).unwrap();
        let o = vm.dump_runtime().unwrap();
        let o = o.as_object().unwrap();
        let keys = vm.heap().own_keys(o);
        assert_eq!(keys.len(), HELPERS.len());
    }
}
