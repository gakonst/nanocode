//! The module API (vm/module.rs): loading, linking, evaluation, namespaces, exports.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};

use super::*;
use crate::compiler::CompileOptions;
use crate::vm::ModuleKind;

/// A loader over `(key, source)` pairs; specifiers are keys. Counts loads.
struct Map {
    files: Vec<(&'static str, &'static str)>,
    loads: Vec<String>,
}

impl ModuleLoader for Map {
    fn resolve(
        &mut self,
        specifier: &str,
        _referrer: Option<&str>,
        _attributes: &[(&str, &str)],
    ) -> Result<String, String> {
        if specifier.starts_with("bad:") {
            return Err(format!("cannot resolve {specifier}"));
        }
        Ok(specifier.to_string())
    }

    fn load(&mut self, key: &str) -> Result<ModuleSource, String> {
        self.loads.push(key.to_string());
        let text = self
            .files
            .iter()
            .find(|f| f.0 == key)
            .ok_or_else(|| format!("no module {key}"))?
            .1;
        Ok(ModuleSource {
            text: text.to_string(),
            kind: if key.ends_with(".ts") {
                ModuleKind::TypeScript
            } else if key.ends_with(".json") {
                ModuleKind::Json
            } else {
                ModuleKind::JavaScript
            },
        })
    }
}

fn map(files: &[(&'static str, &'static str)]) -> Map {
    Map {
        files: files.to_vec(),
        loads: Vec::new(),
    }
}

/// Runs to the end (images in between when `suspend`): the settled promise's outcome.
fn settle(vm: &mut Vm, suspend: bool) -> String {
    loop {
        match vm.run(if suspend { 7 } else { 1_000_000 }) {
            Status::Done(p) => {
                return match vm.promise_state(p) {
                    Some(Some(Ok(v))) => format!("ok {}", vm.display(v)),
                    Some(Some(Err(e))) => format!("err {}", vm.display(e)),
                    other => format!("{other:?}"),
                };
            }
            Status::OutOfFuel if suspend => {
                let opts = vm.options();
                *vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
            }
            Status::OutOfFuel => {}
            s => return format!("{s:?}"),
        }
    }
}

fn eval(vm: &mut Vm, key: &str) -> String {
    assert!(vm.evaluate_module(key).unwrap());
    settle(vm, false)
}

fn export(vm: &mut Vm, key: &str, name: &str) -> String {
    match vm.module_export(key, name).unwrap() {
        Some(v) => vm.display(v),
        None => String::from("-"),
    }
}

/// Calls exported function `name` of `key` with no arguments: its result.
fn call_export(vm: &mut Vm, key: &str, name: &str) -> String {
    let f = vm.module_export(key, name).unwrap().unwrap();
    vm.call(f, Value::UNDEFINED, &[]).unwrap();
    match vm.run(1_000_000) {
        Status::Done(v) => vm.display(v),
        Status::Threw(e) => format!("throw {}", vm.display(e)),
        s => format!("{s:?}"),
    }
}

#[test]
fn live_bindings_and_namespaces() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = map(&[
        (
            "main",
            "import { count, inc } from 'counter';
             import * as ns from 'counter';
             import def from 'counter';
             inc(); inc();
             export const seen = [count, ns.count, def, typeof ns, Object.keys(ns).join(),
                                  Object.prototype.toString.call(ns), this === undefined];
             export { count };",
        ),
        (
            "counter",
            "export let count = 0;
             export function inc() { count++; }
             export default 'd';",
        ),
    ]);
    let key = vm.load_module("main", &mut l).unwrap();
    assert_eq!(key, "main");
    assert_eq!(l.loads, ["main", "counter"]);
    assert_eq!(eval(&mut vm, "main"), "ok undefined");
    assert_eq!(
        export(&mut vm, "main", "seen"),
        "[2,2,\"d\",\"object\",\"count,default,inc\",\"[object Module]\",true]"
    );
    // Live: the re-export reads the binding.
    call_export(&mut vm, "counter", "inc");
    assert_eq!(export(&mut vm, "main", "count"), "3");
    // A second load of the same key loads nothing.
    let mut l2 = map(&[]);
    vm.load_module("main", &mut l2).unwrap();
    assert!(l2.loads.is_empty());
    // A second evaluation runs nothing.
    assert_eq!(eval(&mut vm, "main"), "ok undefined");
    assert_eq!(export(&mut vm, "counter", "count"), "3");
    let ns = vm.module_namespace("counter").unwrap().unwrap();
    vm.define_global("ns", ns).unwrap();
    let f = vm
        .compile(
            b"const r = [];
              try { ns.count = 1 } catch (e) { r.push(e.constructor.name) }
              r.push(Object.isExtensible(ns), Reflect.set(ns, 'count', 1), 'count' in ns,
                     Reflect.deleteProperty(ns, 'count'), delete ns.nope, Reflect.getPrototypeOf(ns),
                     JSON.stringify(Object.getOwnPropertyDescriptor(ns, 'count')),
                     Reflect.defineProperty(ns, 'count', { value: 3 }),
                     Reflect.defineProperty(ns, 'count', { value: 4 }),
                     ns[Symbol.toStringTag]);
              r",
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let g = Value::object(vm.global());
    vm.call(f, g, &[]).unwrap();
    let v = match vm.run(1_000_000) {
        Status::Done(v) => v,
        Status::Threw(e) => panic!("{}", vm.display(e)),
        s => panic!("{s:?}"),
    };
    assert_eq!(
        vm.display(v),
        "[\"TypeError\",false,false,true,false,true,null,\
         \"{\\\"value\\\":3,\\\"writable\\\":true,\\\"enumerable\\\":true,\\\"configurable\\\":false}\",\
         true,false,\"Module\"]"
    );
}

#[test]
fn cycles_and_hoisted_functions() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = map(&[
        (
            "a",
            "import { b, fromB } from 'b';
             export function a() { return 'a' + b(); }
             export const order = [];
             order.push('a', fromB);",
        ),
        (
            "b",
            // `a` runs before a's body: a hoisted function of a module not run yet.
            "import { a, order } from 'a';
             export function b() { return 'b'; }
             export const fromB = a();
             let early;
             try { order } catch (e) { early = e.name }
             export { early };",
        ),
    ]);
    vm.load_module("a", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "a"), "ok undefined");
    assert_eq!(export(&mut vm, "a", "order"), "[\"a\",\"ab\"]");
    assert_eq!(export(&mut vm, "b", "early"), "ReferenceError");
}

#[test]
fn re_exported_namespace_imports() {
    // `import * as ns; export { ns }` re-exports the namespace (as `export * as`): the
    // module's own namespace, imported back from a module that imports it, and two such
    // re-exports of one namespace through `export *` are not ambiguous (test262's
    // instn-star-equality.js and ambiguous-export-bindings).
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = map(&[
        (
            "main",
            "import * as self from 'main';
             import { selfNs } from 'other';
             export * from 'one';
             export * from 'two';
             import { empty } from 'main';
             export const same = [self === selfNs, typeof empty];",
        ),
        (
            "other",
            "import * as selfNs from 'main'; export { selfNs };",
        ),
        ("one", "import * as empty from 'empty'; export { empty };"),
        ("two", "import * as empty from 'empty'; export { empty };"),
        ("empty", ""),
    ]);
    vm.load_module("main", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "main"), "ok undefined");
    assert_eq!(export(&mut vm, "main", "same"), "[true,\"object\"]");
}

#[test]
fn link_and_load_errors() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let cases: &[(&[(&'static str, &'static str)], &str)] = &[
        (
            &[("m", "import { x } from 'n'"), ("n", "export const y = 1")],
            "SyntaxError: Could not find export 'x' in module 'n'",
        ),
        (
            &[
                ("m", "import { x } from 'n'"),
                ("n", "export * from 'o'; export * from 'p'"),
                ("o", "export const x = 1"),
                ("p", "export const x = 2"),
            ],
            "SyntaxError: export 'x' in module 'n' is ambiguous",
        ),
        (
            &[("m", "export { x } from 'n'"), ("n", "export {}")],
            "SyntaxError: Could not find export 'x' in module 'n'",
        ),
        (
            &[("m", "import 'n'"), ("n", "let let = 1")],
            "SyntaxError: ",
        ),
        (&[("m", "import 'n'")], "Error: no module n"),
        (&[("m", "import 'bad:x'")], "Error: cannot resolve bad:x"),
    ];
    for &(files, msg) in cases {
        let mut l = map(files);
        let e = vm.load_module("m", &mut l).unwrap_err();
        let text = vm.display(e);
        assert!(text.starts_with(msg), "{files:?}: {text}");
        // Nothing stays loaded.
        assert!(!vm.has_module("m"));
    }
    // A star export of the same binding twice is not ambiguous; `default` never comes
    // through a star; a namespace leaves ambiguous names out.
    let mut l = map(&[
        (
            "m",
            "import * as ns from 'n'; export const keys = Object.keys(ns).join()",
        ),
        (
            "n",
            "export * from 'o'; export * from 'p'; export * from 'q'",
        ),
        ("o", "export { x, z } from 'q'; export default 1"),
        ("p", "export * from 'q'; export const w = 1"),
        ("q", "export const x = 1, z = 2; export const w = 3"),
    ]);
    vm.load_module("m", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "m"), "ok undefined");
    assert_eq!(export(&mut vm, "m", "keys"), "x,z");
}

#[test]
fn evaluation_errors_are_kept() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = map(&[
        ("m", "import 'n'; export const after = 1"),
        ("n", "export const x = 1; throw new TypeError('boom')"),
        ("k", "import { x } from 'n'"),
    ]);
    vm.load_module("m", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "m"), "err TypeError: boom");
    assert_eq!(eval(&mut vm, "m"), "err TypeError: boom");
    assert_eq!(export(&mut vm, "m", "after"), "-");
    // Another importer of the failed module gets its error too.
    vm.load_module("k", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "k"), "err TypeError: boom");
}

#[test]
fn import_meta_and_typescript() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = map(&[
        (
            "main.ts",
            "import { type T, f } from 'lib.ts';
             import type { U } from 'types.ts';
             import { Only } from 'types.ts';
             let x: T = f();
             export const url = import.meta.url, same = import.meta === (() => import.meta)();
             export { x, type U };",
        ),
        (
            "lib.ts",
            "export type T = number; export function f(): T { return 7 }",
        ),
        ("types.ts", "export interface U {} export type Only = 1;"),
    ]);
    vm.load_module("main.ts", &mut l).unwrap();
    // The type-only imports load nothing.
    assert_eq!(l.loads, ["main.ts", "lib.ts"]);
    assert_eq!(eval(&mut vm, "main.ts"), "ok undefined");
    assert_eq!(export(&mut vm, "main.ts", "url"), "main.ts");
    assert_eq!(export(&mut vm, "main.ts", "same"), "true");
    assert_eq!(export(&mut vm, "main.ts", "x"), "7");
}

#[test]
fn graphs_survive_images_and_suspension() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = map(&[
        (
            "m",
            "import { n, bump } from 'n';
             let s = 0;
             for (let i = 0; i < 50; i++) s += i;
             export const total = s + n;
             export function next() { bump(); return n; }",
        ),
        ("n", "export let n = 1; export function bump() { n++; }"),
    ]);
    vm.load_module("m", &mut l).unwrap();
    assert!(vm.evaluate_module("m").unwrap());
    // Out of fuel many times, through an image each time.
    assert_eq!(settle(&mut vm, true), "ok undefined");
    vm.collect().unwrap();
    let opts = vm.options();
    let mut vm2 = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
    assert_eq!(export(&mut vm2, "m", "total"), "1226");
    assert_eq!(call_export(&mut vm2, "m", "next"), "2");
    assert_eq!(export(&mut vm2, "n", "n"), "2");
    assert_eq!(call_export(&mut vm2, "m", "next"), "3");
    // The original VM is unaffected.
    assert_eq!(export(&mut vm, "n", "n"), "1");
    let _ = vec![0];
}

#[test]
fn synthetic_modules() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let allow = vm.new_host_function(7, "allowHosts").unwrap();
    let secret = Value::string(vm.heap.alloc_str("s3cret").unwrap());
    vm.define_synthetic_module("@sdk/net", &[("allowHosts", allow), ("secret", secret)])
        .unwrap();
    let mut l = map(&[
        (
            "main",
            "import { allowHosts, secret } from '@sdk/net';
             import * as net from '@sdk/net';
             export * from '@sdk/net';
             export function read() { return secret; }
             export function call() { return allowHosts('a', 'b'); }
             export const seen = [typeof allowHosts, secret, Object.keys(net).join(),
                                  net.secret === secret];",
        ),
        ("bad", "import { nope } from '@sdk/net';"),
    ]);
    vm.load_module("main", &mut l).unwrap();
    // The synthetic module is never asked of the loader.
    assert_eq!(l.loads, ["main"]);
    assert_eq!(eval(&mut vm, "main"), "ok undefined");
    assert_eq!(
        export(&mut vm, "main", "seen"),
        "[\"function\",\"s3cret\",\"allowHosts,secret\",true]"
    );
    assert_eq!(export(&mut vm, "main", "secret"), "s3cret");
    // The exports are host functions like any other.
    let f = vm.module_export("main", "call").unwrap().unwrap();
    vm.call(f, Value::UNDEFINED, &[]).unwrap();
    let Status::HostCall { call_id, fn_id } = vm.run(1_000_000) else {
        panic!("a host call")
    };
    assert_eq!(fn_id, 7);
    assert_eq!(vm.host_call(call_id).unwrap().args.len(), 2);
    vm.resolve_host_call(call_id, Ok(Value::TRUE)).unwrap();
    assert!(matches!(vm.run(1_000_000), Status::Done(v) if v == Value::TRUE));
    // Live: importers see a new value.
    assert!(
        vm.set_module_export("@sdk/net", "secret", Value::int(5))
            .unwrap()
    );
    assert_eq!(call_export(&mut vm, "main", "read"), "5");
    assert!(
        !vm.set_module_export("@sdk/net", "nope", Value::int(5))
            .unwrap()
    );
    assert!(!vm.set_module_export("main", "seen", Value::int(5)).unwrap());
    assert!(!vm.set_module_export("none", "x", Value::int(5)).unwrap());
    // Evaluating it runs nothing.
    assert_eq!(eval(&mut vm, "@sdk/net"), "ok undefined");
    let e = vm.load_module("bad", &mut l).unwrap_err();
    assert_eq!(
        vm.display(e),
        "SyntaxError: Could not find export 'nope' in module '@sdk/net'"
    );
    let e = vm.define_synthetic_module("@sdk/net", &[]).unwrap_err();
    assert_eq!(vm.display(e), "Error: module '@sdk/net' is loaded already");
    let e = vm
        .define_synthetic_module("dup", &[("a", Value::NULL), ("a", Value::NULL)])
        .unwrap_err();
    assert_eq!(vm.display(e), "SyntaxError: duplicate export 'a'");
    // Images carry it, live bindings included.
    vm.collect().unwrap();
    let mut vm = Vm::from_bytes(vm.image().to_vec(), VmOptions::default()).unwrap();
    assert!(
        vm.set_module_export("@sdk/net", "secret", Value::int(6))
            .unwrap()
    );
    assert_eq!(call_export(&mut vm, "main", "read"), "6");
}

/// A loader that resolves relative specifiers against the referrer and records the
/// requests it saw.
struct Rel {
    files: Vec<(&'static str, &'static str)>,
    resolves: Vec<String>,
}

impl ModuleLoader for Rel {
    fn resolve(
        &mut self,
        specifier: &str,
        referrer: Option<&str>,
        _attributes: &[(&str, &str)],
    ) -> Result<String, String> {
        self.resolves
            .push(format!("{specifier}<{}", referrer.unwrap_or("-")));
        let key = match (specifier.strip_prefix("./"), referrer) {
            (Some(rest), Some(r)) => match r.rsplit_once('/') {
                Some((dir, _)) => format!("{dir}/{rest}"),
                None => rest.to_string(),
            },
            _ => specifier.to_string(),
        };
        if self.files.iter().any(|f| f.0 == key) {
            Ok(key)
        } else {
            Err(format!("cannot find {specifier}"))
        }
    }

    fn load(&mut self, key: &str) -> Result<ModuleSource, String> {
        let text = self.files.iter().find(|f| f.0 == key).unwrap().1;
        Ok(ModuleSource {
            text: text.to_string(),
            kind: if key.ends_with(".json") {
                ModuleKind::Json
            } else {
                ModuleKind::JavaScript
            },
        })
    }
}

/// Runs a script that sets `globalThis.out` (a promise's outcome or anything), answering
/// every import with `l`: `out` when nothing is left to do.
fn run_imports(vm: &mut Vm, l: &mut Rel, src: &str) -> String {
    let f = vm
        .compile(src.as_bytes(), &CompileOptions::script("dir/script.js"))
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    loop {
        match vm.run(1_000_000) {
            Status::Idle => {
                let calls = vm.new_host_calls();
                assert!(!calls.is_empty(), "idle without new calls");
                for c in calls {
                    vm.answer_import(c, l).unwrap();
                }
            }
            Status::Done(_) if !vm.pending_host_calls().is_empty() => {
                for c in vm.new_host_calls() {
                    vm.answer_import(c, l).unwrap();
                }
            }
            Status::Done(_) => break,
            s => panic!("{s:?}"),
        }
    }
    let out = vm.global_path("out").unwrap_or(Value::UNDEFINED);
    vm.display(out)
}

const IMPORT_FILES: &[(&str, &str)] = &[
    (
        "dir/a.js",
        "export const x = 1; export default 'A'; import * as b from './b.js'; export { b };",
    ),
    ("dir/b.js", "export let y = 2; log.push('b ran');"),
    (
        "dir/c.js",
        "export async function go() { const m = await import('./a.js'); return m.x; }",
    ),
    (
        "dir/throws.js",
        "log.push('throws ran'); throw new RangeError('boom');",
    ),
    ("dir/missing.js", "import { z } from './b.js';"),
    ("dir/syntax.js", "export const = 1;"),
    (
        "dir/then.js",
        "export function then(resolve) { resolve('thenable'); }",
    ),
];

#[test]
fn dynamic_import_through_the_loader() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.set_dynamic_import(true).unwrap();
    let mut l = Rel {
        files: IMPORT_FILES.to_vec(),
        resolves: Vec::new(),
    };
    let out = run_imports(
        &mut vm,
        &mut l,
        "globalThis.log = [];
         const p = import('./a.js');
         log.push(String(p instanceof Promise));
         p.then(ns => { log.push('ns ' + [ns.x, ns.default, ns.b.y, Object.keys(ns)].join());
                        return import('./a.js').then(ns2 => log.push(String(ns2 === ns))); })
          .then(() => import('./c.js')).then(c => c.go()).then(x => log.push('c ' + x))
          .then(() => { globalThis.out = log; });",
    );
    assert_eq!(
        out,
        "[\"true\",\"b ran\",\"ns 1,A,2,b,default,x\",\"true\",\"c 1\"]"
    );
    // The referrers: the script's name, then the module's key (from an async function).
    assert_eq!(
        l.resolves,
        [
            "./a.js<dir/script.js",
            "./b.js<dir/a.js",
            "./a.js<dir/script.js",
            "./c.js<dir/script.js",
            "./a.js<dir/c.js",
        ]
    );
}

#[test]
fn dynamic_import_errors_reject() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.set_dynamic_import(true).unwrap();
    let mut l = Rel {
        files: IMPORT_FILES.to_vec(),
        resolves: Vec::new(),
    };
    let out = run_imports(
        &mut vm,
        &mut l,
        "globalThis.log = [];
         const show = (e) => e instanceof Error ? e.name + ': ' + e.message : String(e);
         const tries = [
           () => import('./nowhere.js'),
           () => import('./throws.js'),
           () => import('./throws.js'),
           () => import('./missing.js'),
           () => import('./syntax.js'),
           () => import('./b.js', 5),
           () => import('./b.js', { with: { type: 'json' } }),
           () => import('./b.js', { with: { type: 1 } }),
           () => import({ toString() { throw new TypeError('spec'); } }),
           () => import('./then.js'),
         ];
         let first;
         tries.reduce((p, f, i) => p.then(() => f()).then(
             (v) => log.push(i + ' ok ' + (typeof v === 'string' ? v : typeof v)),
             (e) => { if (i === 1) first = e; else if (i === 2) log.push('same ' + (e === first));
                      log.push(i + ' ' + show(e)); }),
           Promise.resolve()).then(() => { globalThis.out = log; });",
    );
    assert_eq!(
        out,
        "[\"0 Error: cannot find ./nowhere.js\",\"throws ran\",\"1 RangeError: boom\",\
          \"same true\",\"2 RangeError: boom\",\
          \"3 SyntaxError: Could not find export 'z' in module 'dir/b.js'\",\
          \"4 SyntaxError: variable name expected at dir/syntax.js:1:14\",\
          \"5 TypeError: options must be an object\",\
          \"6 TypeError: module 'dir/b.js' is not a JSON module\",\
          \"7 TypeError: module attribute values must be strings\",\
          \"8 TypeError: spec\",\"9 ok thenable\"]"
    );
}

#[test]
fn dynamic_import_off_rejects_as_without_modules() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = Rel {
        files: IMPORT_FILES.to_vec(),
        resolves: Vec::new(),
    };
    let out = run_imports(
        &mut vm,
        &mut l,
        "import('./a.js').catch(e => { globalThis.out = e.name + ': ' + e.message; });",
    );
    assert_eq!(out, "ReferenceError: could not load module './a.js'");
    // Turned on and off again.
    vm.set_dynamic_import(true).unwrap();
    vm.set_dynamic_import(false).unwrap();
    let out = run_imports(
        &mut vm,
        &mut l,
        "import('./a.js').catch(e => { globalThis.out = e.name; });",
    );
    assert_eq!(out, "ReferenceError");
    assert!(l.resolves.is_empty());
}

#[test]
fn dynamic_import_survives_images() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.set_dynamic_import(true).unwrap();
    vm.collect().unwrap();
    let mut vm = Vm::from_bytes(vm.image().to_vec(), VmOptions::default()).unwrap();
    let f = vm
        .compile(
            b"globalThis.log = []; import('./b.js').then(ns => { globalThis.out = ns.y; });",
            &CompileOptions::script("dir/script.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(vm.run(1_000_000), Status::Idle));
    let calls = vm.new_host_calls();
    assert_eq!(
        vm.import_request(calls[0]),
        Some(("./b.js".to_string(), "dir/script.js".to_string()))
    );
    // An image between the call and its answer.
    vm.collect().unwrap();
    let mut vm = Vm::from_bytes(vm.image().to_vec(), VmOptions::default()).unwrap();
    let mut l = Rel {
        files: IMPORT_FILES.to_vec(),
        resolves: Vec::new(),
    };
    vm.answer_import(calls[0], &mut l).unwrap();
    assert!(matches!(vm.run(1_000_000), Status::Done(_)));
    let out = vm.global_path("out").unwrap();
    assert_eq!(vm.display(out), "2");
    assert_eq!(
        vm.answer_import(calls[0], &mut l),
        Err(VmError::UnknownHostCall)
    );
}

#[test]
fn json_modules() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = map(&[
        (
            "m",
            "import data from 'd.json' with { type: 'json' };
             import * as ns from 'd.json' with { type: 'json' };
             export const seen = [data.a, data.list.length, ns.default === data,
                                  Object.keys(ns).join(), Object.getPrototypeOf(data) === Object.prototype];
             data.a = 5;",
        ),
        ("d.json", "{\"a\": 1, \"list\": [1, 2, 3]}"),
        ("bad.json", "{\"a\": "),
        ("plain", "export const x = 1"),
        ("n1", "import x from 'plain' with { type: 'json' }"),
        ("n2", "import x from 'd.json'"),
        ("n3", "import x from 'd.json' with { type: 'css' }"),
        ("n4", "import x from 'd.json' with { type: 'json', other: 'x' }"),
        ("n5", "import x from 'bad.json' with { type: 'json' }"),
        ("n6", "import { a } from 'd.json' with { type: 'json' }"),
    ]);
    vm.load_module("m", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "m"), "ok undefined");
    assert_eq!(export(&mut vm, "m", "seen"), "[1,3,true,\"default\",true]");
    // The parsed value is one object, shared by its importers (and mutable, as in browsers).
    assert_eq!(
        export(&mut vm, "d.json", "default"),
        "{\"a\":5,\"list\":[1,2,3]}"
    );
    for (key, msg) in [
        ("n1", "TypeError: module 'plain' is not a JSON module"),
        (
            "n2",
            "TypeError: module 'd.json' is JSON: import it with type 'json'",
        ),
        ("n3", "TypeError: unsupported module type 'css'"),
        ("n4", "SyntaxError: unsupported import attribute 'other'"),
        ("n5", "SyntaxError: "),
        (
            "n6",
            "SyntaxError: Could not find export 'a' in module 'd.json'",
        ),
    ] {
        let e = vm.load_module(key, &mut l).unwrap_err();
        let text = vm.display(e);
        assert!(text.starts_with(msg), "{key}: {text}");
        assert!(!vm.has_module(key));
    }
    // Duplicate attribute keys are an early error; a line break may precede `with`.
    let opts = CompileOptions::module("x");
    let dup = "import x from 'd.json' with { type: 'json', type: 'json' }";
    assert!(crate::compiler::compile_module(vm.heap_mut(), dup.as_bytes(), &opts).is_err());
    let nl = "import x from 'd.json'\n  with { type: 'json' };";
    assert!(crate::compiler::compile_module(vm.heap_mut(), nl.as_bytes(), &opts).is_ok());
}

#[test]
fn json_modules_through_import() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.set_dynamic_import(true).unwrap();
    let mut l = Rel {
        files: vec![
            ("dir/d.json", "[1, {\"b\": true}]"),
            ("dir/m.js", "export const x = 1"),
        ],
        resolves: Vec::new(),
    };
    let out = run_imports(
        &mut vm,
        &mut l,
        "globalThis.log = [];
         import('./d.json', { with: { type: 'json' } })
           .then(ns => log.push(JSON.stringify(ns.default)))
           .then(() => import('./d.json'))
           .catch(e => log.push(e.name + ': ' + e.message))
           .then(() => import('./m.js', { with: { type: 'json' } }))
           .catch(e => log.push(e.name))
           .then(() => import('./m.js', { with: { kind: 'x' } }))
           .catch(e => log.push(e.name))
           .then(() => { globalThis.out = log; });",
    );
    assert_eq!(
        out,
        "[\"[1,{\\\"b\\\":true}]\",\"TypeError: module 'dir/d.json' is JSON: import it with type 'json'\",\"TypeError\",\"SyntaxError\"]"
    );
}

#[test]
fn top_level_await() {
    for suspend in [false, true] {
        let mut vm = Vm::new(VmOptions::default()).unwrap();
        let mut l = map(&[
            (
                "main",
                "import { x } from 'slow';
                 import { y } from 'sync';
                 import 'fast';
                 globalThis.log.push('main ' + x + y);
                 export const z = await x + 1;",
            ),
            (
                "slow",
                "import 'log';
                 log.push('slow start');
                 export let x = 0;
                 for await (const v of [Promise.resolve(1), 2]) x += v;
                 await null; await null;
                 log.push('slow end');",
            ),
            (
                "sync",
                "import 'log'; log.push('sync'); export const y = 'y';",
            ),
            (
                "fast",
                "import 'log'; log.push('fast start'); await 0; log.push('fast end');",
            ),
            ("log", "globalThis.log = [];"),
        ]);
        vm.load_module("main", &mut l).unwrap();
        assert!(vm.evaluate_module("main").unwrap());
        let r = settle(&mut vm, suspend);
        let log = vm.global_path("log").unwrap();
        assert_eq!(r, "ok undefined", "{}", vm.display(log));
        // Async siblings interleave; their importer runs once both are done.
        assert_eq!(
            vm.display(log),
            "[\"slow start\",\"sync\",\"fast start\",\"fast end\",\"slow end\",\"main 3y\"]"
        );
        assert_eq!(export(&mut vm, "main", "z"), "4");
        // Evaluated: a second evaluation gives a settled promise at once.
        assert_eq!(eval(&mut vm, "main"), "ok undefined");
        assert_eq!(eval(&mut vm, "fast"), "ok undefined");
    }
}

#[test]
fn top_level_await_errors() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let mut l = map(&[
        ("m", "import 'a'; export const after = 1;"),
        ("a", "await 0; throw new RangeError('late');"),
        ("k", "import 'm';"),
        ("ok", "export const v = await new Promise(r => r('v'));"),
        ("await_id", "function f() { await 1 }"),
        ("await_arrow", "const f = () => await 1;"),
    ]);
    vm.load_module("m", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "m"), "err RangeError: late");
    assert_eq!(export(&mut vm, "m", "after"), "-");
    vm.load_module("k", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "k"), "err RangeError: late");
    assert_eq!(eval(&mut vm, "m"), "err RangeError: late");
    vm.load_module("ok", &mut l).unwrap();
    assert_eq!(eval(&mut vm, "ok"), "ok undefined");
    assert_eq!(export(&mut vm, "ok", "v"), "v");
    // `await` outside the module's own top level is no operator.
    for key in ["await_id", "await_arrow"] {
        let e = vm.load_module(key, &mut l).unwrap_err();
        assert!(vm.display(e).starts_with("SyntaxError"), "{key}");
    }
}

#[test]
fn top_level_await_in_cycles_and_imports() {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.set_dynamic_import(true).unwrap();
    let mut l = Rel {
        files: vec![
            (
                "dir/a.js",
                "import { b } from './b.js'; export const a = 'a'; log.push('a ' + b);",
            ),
            (
                "dir/b.js",
                "import { a } from './a.js'; log.push('b'); await 0; export const b = 'b';
                 export function getA() { return a; }",
            ),
            (
                "dir/waits.js",
                "log.push('waits'); export default await import('./a.js').then(m => m.a);",
            ),
        ],
        resolves: Vec::new(),
    };
    let out = run_imports(
        &mut vm,
        &mut l,
        "globalThis.log = [];
         import('./waits.js')
           .then(ns => log.push('done ' + ns.default))
           .then(() => import('./b.js'))
           .then(ns => log.push('b.getA ' + ns.getA()))
           .then(() => { globalThis.out = log; });",
    );
    assert_eq!(out, "[\"waits\",\"b\",\"a b\",\"done a\",\"b.getA a\"]");
}
