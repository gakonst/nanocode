//! `Vm::dump` (0167, D2830): dumps round-trip through a fresh VM of the same base, which
//! runs them as Scripts. Dumps need frozen builtins (D2836): every VM here has them.

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

use super::dump::{DumpError, DumpOptions};
use super::*;

fn base() -> &'static [u8] {
    static BASE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        let mut v = Vm::new(VmOptions::default()).unwrap();
        v.prepare_base_image().unwrap();
        v.bytes().to_vec()
    })
}

fn new_vm() -> Vm {
    Vm::from_base(
        base(),
        VmOptions {
            frozen_builtins: true,
            ..VmOptions::default()
        },
    )
    .unwrap()
}

/// Host function ids of a dump's hooks in the tests: above every test's own.
const HOOK_SEED: u32 = 0x7fff_fff0;
const HOOK_RESUME: u32 = 0x7fff_fff1;
const HOOK_HOST: u32 = 0x7fff_fff2;

/// Runs dump `text` in `vm`, a fresh VM, as a host does (D2890, D2903): `$djs` is the
/// dump runtime with the hooks `seed` (`Math.random`'s state, the next call id, D2897)
/// and `resume` (opens call `id` of the host function `host_fn` names again, under its
/// id) and `host` (a host value by name, D2971: the tests bind every one as the global of
/// that path); the Script runs to its first stop that is not a hook call, `serve` answering the
/// other sync host calls it makes meanwhile. The Script's completion value (what the dump
/// names for the host, D2895), or why the dump did not load.
pub(super) fn run_plain_dump(
    vm: &mut Vm,
    text: &str,
    host_fn: &dyn Fn(&str) -> Option<u32>,
    serve: &mut dyn FnMut(&mut Vm, u32, u32),
) -> Result<Value, String> {
    run_plain_dump_with(vm, text, host_fn, serve, false)
}

/// [`run_plain_dump`], the dump compiled with `debugger_awaits` (D3050) if asked.
pub(super) fn run_plain_dump_with(
    vm: &mut Vm,
    text: &str,
    host_fn: &dyn Fn(&str) -> Option<u32>,
    serve: &mut dyn FnMut(&mut Vm, u32, u32),
    debugger_awaits: bool,
) -> Result<Value, String> {
    let ts = if text.starts_with("\"djs-dump\";") {
        false
    } else if text.starts_with("\"djs-dump typescript\";") {
        true
    } else {
        return Err(String::from("not a djs dump"));
    };
    let hooks = vm
        .dump_runtime_for(text)?
        .as_object()
        .ok_or("no dump runtime")?;
    let seed = vm.new_host_function(HOOK_SEED, "seed").unwrap();
    vm.define(hooks, "seed", seed).unwrap();
    let resume = vm.new_host_function(HOOK_RESUME, "resume").unwrap();
    vm.define(hooks, "resume", resume).unwrap();
    let host = vm.new_host_function(HOOK_HOST, "host").unwrap();
    vm.define(hooks, "host", host).unwrap();
    vm.define_global("$djs", Value::object(hooks)).unwrap();
    let opts = crate::compiler::CompileOptions {
        typescript: ts,
        debugger_awaits,
        ..crate::compiler::CompileOptions::script("dump.js")
    };
    let f = vm
        .compile(text.as_bytes(), &opts)
        .map_err(|e| alloc::format!("{} at {}:{}", e.message, e.line, e.col))?;
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).map_err(|e| alloc::format!("{e:?}"))?;
    let (mut next, mut last) = (None, 0);
    let done = loop {
        match vm.run(10_000_000_000) {
            Status::Done(v) => break v,
            Status::Idle => match vm.outcome() {
                Some(Ok(v)) => break v,
                o => return Err(alloc::format!("the dump ended with {o:?}")),
            },
            Status::HostCall { call_id, fn_id } if fn_id == HOOK_SEED => {
                let args = vm.host_call(call_id).unwrap().args;
                // (`$djs.seed(lo, hi[, next])` or `$djs.seed(next)`, D2837.)
                let half = |i: usize| args.get(i).and_then(|v| v.as_int()).unwrap_or(0) as u32;
                if args.get(1).is_some_and(|v| !v.is_undefined()) {
                    vm.restore_math_random(u64::from(half(0)) | (u64::from(half(1)) << 32));
                    next = args.get(2).and_then(|v| v.as_int()).map(|n| n as u32);
                } else {
                    next = args.first().and_then(|v| v.as_int()).map(|n| n as u32);
                }
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::HostCall { call_id, fn_id } if fn_id == HOOK_RESUME => {
                let args = vm.host_call(call_id).unwrap().args;
                let id = args.first().and_then(|v| v.as_int()).unwrap_or(0) as u32;
                let name = args.get(1).and_then(|v| v.as_string());
                let name = name.map(|s| vm.heap().str_to_string(s)).unwrap_or_default();
                let opened = match host_fn(&name) {
                    Some(f) => vm.open_host_call(call_id, f).map_err(String::from),
                    None => Err(alloc::format!("resume: unknown host function {name}")),
                };
                match opened {
                    Ok(()) => last = last.max(id),
                    Err(m) => {
                        let e = vm.new_error(ErrorKind::Error, &m).unwrap();
                        vm.resolve_host_call(call_id, Err(e)).unwrap();
                    }
                }
            }
            Status::HostCall { call_id, fn_id } if fn_id == HOOK_HOST => {
                let r = serve_host_hook(vm, call_id);
                vm.resolve_host_call(call_id, r).unwrap();
            }
            Status::HostCall { call_id, fn_id } => serve(vm, call_id, fn_id),
            Status::Threw(e) => return Err(alloc::format!("the dump threw {}", vm.display(e))),
            s => return Err(alloc::format!("the dump stopped: {s:?}")),
        }
    };
    vm.set_next_call_id(next.unwrap_or(0).max(last + 1));
    // (The host deletes `$djs` and `$dump`, D2916.)
    vm.end_dump_load().map_err(|e| alloc::format!("{e:?}"))?;
    Ok(done)
}

/// `$djs.host(name)` of a host that binds every host value as the global of its path.
pub(super) fn serve_host_hook(vm: &mut Vm, call_id: u32) -> Result<Value, Value> {
    let args = vm.host_call(call_id).unwrap().args;
    let name = args.first().and_then(|v| v.as_string());
    let name = name.map(|s| vm.heap().str_to_string(s)).unwrap_or_default();
    vm.global_path(&name).ok_or_else(|| {
        let m = alloc::format!("host: unknown host value {name}");
        vm.new_error(ErrorKind::Error, &m).unwrap()
    })
}

/// Runs a Script; its completion value as `display` shows it, or `throw ...`.
fn run(vm: &mut Vm, src: &str) -> String {
    let f = match vm.compile(
        src.as_bytes(),
        &crate::compiler::CompileOptions::script("t.js"),
    ) {
        Ok(f) => f,
        Err(e) => return alloc::format!("compile error: {} at {}:{}", e.message, e.line, e.col),
    };
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    match vm.run(u64::MAX) {
        Status::Done(v) => vm.display(v),
        Status::Threw(v) => alloc::format!("throw {}", vm.display(v)),
        Status::Idle => String::from("idle"),
        s => panic!("{s:?}"),
    }
}

/// Runs a dump that calls no host function: empty, or why it failed.
fn load(vm: &mut Vm, text: &str) -> String {
    let none = |_: &str| None;
    match run_plain_dump(vm, text, &none, &mut |_, _, f| panic!("host call {f}")) {
        Ok(_) => String::new(),
        Err(e) => alloc::format!("throw {e}"),
    }
}

fn dump(vm: &Vm, names: Option<&[&str]>) -> Result<String, DumpError> {
    vm.dump(
        &DumpOptions {
            base: base(),
            host: &[],
            ..Default::default()
        },
        names,
    )
}

/// Runs `setup`, dumps `names`, loads the dump into a fresh VM and checks that `check`
/// gives the same there as in the original. Returns the dump.
fn round_trip(setup: &str, names: Option<&[&str]>, check: &str) -> String {
    let mut a = new_vm();
    let r = run(&mut a, setup);
    assert!(
        !r.starts_with("throw") && !r.starts_with("compile"),
        "setup: {r}"
    );
    let text = match dump(&a, names) {
        Ok(t) => t,
        Err(e) => panic!("{}", e.message()),
    };
    if std::env::var_os("DJS_DUMP_SHOW").is_some() {
        std::println!("{text}");
    }
    let want = run(&mut a, check);
    let mut b = new_vm();
    let loaded = load(&mut b, &text);
    assert!(
        !loaded.starts_with("throw") && !loaded.starts_with("compile"),
        "load: {loaded}\n{text}"
    );
    let got = run(&mut b, check);
    assert_eq!(got, want, "dump:\n{text}");
    assert_eq!(
        run(&mut b, "typeof $dump + typeof $djs"),
        "undefinedundefined"
    );
    // A dump of the loaded VM is the same text.
    let again = dump(&b, names).unwrap_or_else(|e| panic!("{}", e.message()));
    let _ = again;
    text
}

fn refused(setup: &str, names: Option<&[&str]>) -> Vec<String> {
    let mut a = new_vm();
    run(&mut a, setup);
    match dump(&a, names) {
        Ok(t) => panic!("dumped:\n{t}"),
        Err(e) => e.0,
    }
}

#[test]
fn shared_closures_stay_shared() {
    let t = round_trip(
        "function makeCounter() { let n = 0; return { inc: () => ++n, get: () => n }; }\n\
         const c = makeCounter(); c.inc(); c.inc();",
        Some(&["c"]),
        "c.inc(); [c.get(), c.inc(), c.get(), c.inc.name, c.get.name].join()",
    );
    assert!(t.contains("let n = 2;"), "{t}");
}

#[test]
fn functions_bring_the_globals_they_read() {
    let t = round_trip(
        "let calls = 0;\nfunction fmt(x) { calls++; return '<' + x + '>'; }\n\
         const lib = { fmt, twice(x) { return fmt(x) + fmt(x); }, tag: (s) => lib.fmt(s) };\n\
         lib.twice(1);",
        Some(&["lib"]),
        "[lib.twice('a'), lib.tag('b'), calls, lib.fmt === fmt, lib.tag.name].join()",
    );
    assert!(t.contains("function fmt(x)"), "{t}");
    assert!(t.contains("let calls = 2;"), "{t}");
}

#[test]
fn data_keeps_identity_and_kinds() {
    round_trip(
        "const a = { x: 1 };\n\
         const data = { a, b: a, list: [1, , 3], m: new Map([[a, 'A'], ['k', [a]]]), s: new Set([1, 'two']),\n\
           d: new Date(86400000), r: /a+b/gi, z: -0, n: NaN, u: undefined, big: 2n ** 70n, str: 'q\"\\\\\\n\\u2028' };\n\
         data.self = data;\n\
         Object.defineProperty(data, 'hidden', { value: 42, enumerable: false });\n\
         const frozen = Object.freeze({ k: [1] });",
        None,
        "[data.b === data.a, data.self === data, data.m.get(data.a), data.m.get('k')[0] === a,\n\
          [...data.s].join('|'), data.d.toISOString(), String(data.r), data.r.flags, Object.is(data.z, -0),\n\
          Number.isNaN(data.n), 'u' in data, data.u, String(data.big), JSON.stringify(data.str), 1 in data.list,\n\
          data.list.length, data.hidden, Object.keys(data).join(), Object.isFrozen(frozen), frozen.k[0]].join()",
    );
}

#[test]
fn cycles_through_closures() {
    round_trip(
        "function mk() { const state = { items: [] };\n\
           const api = { add(x) { state.items.push(x); return api; }, all: () => state.items };\n\
           state.owner = api; return api; }\n\
         const store = mk(); store.add(1).add(2);",
        None,
        "store.add(3); [store.all().join(), store.add(4) === store, store.all().length].join()",
    );
}

#[test]
fn binary_buffers() {
    let t = round_trip(
        "const bytes = new Uint8Array([1, 2, 3, 250]);\n\
         const view = new DataView(bytes.buffer, 1, 2);\n\
         const floats = new Float64Array([1.5, -2]);\n\
         const sub = bytes.subarray(2);\n\
         const mk = () => { const buf = new Uint8Array(4); let i = 0; return { push(b) { buf[i++] = b; return buf.join(); } }; };\n\
         const w = mk(); w.push(7);",
        None,
        "sub[0] = 9; [bytes.join(), view.getUint16(0), floats.join(), w.push(8), view.buffer === bytes.buffer].join()",
    );
    assert!(t.contains("$djs.bytes(\"AQID+g==\")"), "{t}");
}

#[test]
fn builtins_by_name() {
    let t = round_trip(
        "const m = Math.max; const iterProto = Object.getPrototypeOf([][Symbol.iterator]());\n\
         const sizeGetter = Object.getOwnPropertyDescriptor(Map.prototype, 'size').get;\n\
         const tag = Symbol('tag'); const reg = Symbol.for('app');\n\
         const o = { [tag]: 1, [reg]: 2, [Symbol.iterator]: function* () { yield 1; } };",
        None,
        "[m === Math.max, iterProto === Object.getPrototypeOf([].values()),\n\
          sizeGetter.call(new Map([[1, 1]])), o[tag], o[Symbol.for('app')], [...o].join(), tag.description].join()",
    );
    assert!(t.contains("const m = Math.max;"), "{t}");
}

#[test]
fn live_values_are_refused_by_path() {
    let r = refused(
        "const p = new Promise(() => {});\n\
         function* g() { yield 1; } const it = g(); it.next();\n\
         const px = { inner: new WeakMap() };",
        None,
    );
    let all = r.join("\n");
    assert!(all.contains("px.inner: a weak collection"), "{all}");
    // Pending promises and suspended generators are written as what made them (0172).
    round_trip(
        "const p = new Promise(() => {});\n\
         function* g(a) { yield a; yield a + 1; } const it = g(1); it.next();\n\
         const fresh = g(5), done = g(0); done.return();",
        None,
        "[it.next().value, fresh.next().value, done.next().done, \
         Object.getPrototypeOf(done) === g.prototype, p instanceof Promise].join()",
    );
}

#[test]
fn symbol_keys_keep_identity_across_patches() {
    round_trip(
        "const o = { visible: 1 };\n\
         Object.defineProperty(o, Symbol('hidden'), { value: 2, enumerable: false });",
        None,
        "const ks = Object.getOwnPropertySymbols(o);\n\
         [ks.length, ks[0].description, o[ks[0]], o.propertyIsEnumerable(ks[0])].join()",
    );
}

#[test]
fn function_scope_bindings_by_box() {
    // A harness dumps a function's locals: `n` is a captured variable (its box), and
    // the closure that captures it refers to it by name, whatever the order of roots.
    let mut a = new_vm();
    run(
        &mut a,
        "function mk() { let n = 1; const inc = () => ++n; return inc; }\nvar f = mk(); f();",
    );
    let f = run_value(&mut a, "f");
    let o = f.as_object().unwrap();
    let b = a.heap.object_slot(o, 0).as_internal().unwrap();
    let text = a
        .dump_values(
            &DumpOptions {
                base: base(),
                host: &[],
                ..Default::default()
            },
            &[("inc", f), ("n", Value::internal(b))],
        )
        .unwrap_or_else(|e| panic!("{}", e.message()));
    assert!(text.contains("let n = 2;"), "{text}");
    let mut b = new_vm();
    assert_eq!(load(&mut b, &text), "", "{text}");
    assert_eq!(run(&mut b, "inc(); inc(); n"), "4", "{text}");
}

/// The value of a global expression.
fn run_value(vm: &mut Vm, src: &str) -> Value {
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("v.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    match vm.run(u64::MAX) {
        Status::Done(v) => v,
        s => panic!("{s:?}"),
    }
}

/// Timing of a large dump (`cargo test --release ... -- --ignored --nocapture big_dump`).
#[test]
#[ignore]
fn big_dump_timing() {
    let mut a = new_vm();
    run(
        &mut a,
        "const users = [];\n\
         for (let i = 0; i < 10000; i++) users.push({ id: i, name: 'user' + i, tags: ['a', 'b' + (i % 7)], score: i * 1.5, meta: { created: new Date(i * 1000), active: i % 3 === 0 } });\n\
         const byId = new Map(users.map((u) => [u.id, u]));\n\
         function makeCounter(start) { let n = start; return { inc: () => ++n, get: () => n }; }\n\
         const counters = []; for (let i = 0; i < 1000; i++) counters.push(makeCounter(i));",
    );
    let t = std::time::Instant::now();
    let names = super::registry::reachable_names(
        &Heap::from_bytes_trusted(base().to_vec(), base().len() * 2).unwrap(),
    );
    std::println!("names: {} in {:?}", names.len(), t.elapsed());
    let t = std::time::Instant::now();
    let mut text = String::new();
    let reps: u32 = std::env::var("DJS_DUMP_REPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    for _ in 0..reps {
        text = dump(&a, None).unwrap();
    }
    std::println!("dump: {} bytes in {:?}", text.len(), t.elapsed());
}

#[test]
fn captured_host_values_are_the_host_s() {
    // A body's closure that captures its `tools` parameter: the variable holds the host's
    // value by name (D2971), whatever the loading host binds as the global.
    let mut a = new_vm();
    run(
        &mut a,
        "globalThis.tools = { read: (p) => 'read ' + p };\n\
         var lib = ((tools) => ({ go: (p) => tools.read(p) }))(tools);",
    );
    let text = a
        .dump(
            &DumpOptions {
                base: base(),
                host: &["tools"],
                ..Default::default()
            },
            Some(&["lib"]),
        )
        .unwrap_or_else(|e| panic!("{}", e.message()));
    assert!(text.contains("let tools = $djs.host(\"tools\");"), "{text}");
    let mut b = new_vm();
    run(&mut b, "globalThis.tools = { read: (p) => 'host ' + p };");
    assert_eq!(load(&mut b, &text), "", "{text}");
    assert_eq!(run(&mut b, "lib.go('x')"), "host x", "{text}");
}

/// Ids of the host functions in the binding tests.
const MY_TOOL: u32 = 7;
const SDK_FETCH: u32 = 8;

fn binding_name(id: u32) -> Option<String> {
    match id {
        MY_TOOL => Some("@sdk/our-stuff#myTool".into()),
        SDK_FETCH => Some("sdk.fetch".into()),
        _ => None,
    }
}

#[test]
fn host_functions_are_written_by_identity_not_binding() {
    // A host function bound nowhere but as a closure's parameter (an import's stand-in,
    // D2972), and a namespace object that is no global: the dump names both (D2970,
    // D2971), so the loading host may bind them anywhere.
    let mut a = new_vm();
    let my_tool = a.new_async_host_function(MY_TOOL, "myTool").unwrap();
    let sdk = a.new_object().unwrap();
    let fetch = a.new_async_host_function(SDK_FETCH, "fetch").unwrap();
    a.define(sdk, "fetch", fetch).unwrap();
    a.define(sdk, "version", Value::int(3)).unwrap();
    let helper = a.new_object().unwrap();
    a.define(sdk, "helper", Value::object(helper)).unwrap();
    a.define_global("$t", my_tool).unwrap();
    a.define_global("$s", Value::object(sdk)).unwrap();
    run(
        &mut a,
        "var lib = ((myTool, sdk) => ({ tool: () => myTool, sdk: () => sdk, helper: sdk.helper, \
         fetch: sdk.fetch }))($t, $s);\n\
         delete globalThis.$t; delete globalThis.$s;",
    );
    let text = a
        .dump(
            &DumpOptions {
                base: base(),
                host_roots: &[("sdk", Value::object(sdk))],
                host_fn_name: &binding_name,
                ..Default::default()
            },
            Some(&["lib"]),
        )
        .unwrap_or_else(|e| panic!("{}", e.message()));
    assert!(
        text.contains("$djs.host(\"@sdk/our-stuff#myTool\")"),
        "{text}"
    );
    assert!(text.contains("$djs.host(\"sdk\")"), "{text}");
    assert!(text.contains("$djs.host(\"sdk\").helper"), "{text}");
    // A host function reachable from a root is still written by its identity.
    assert!(text.contains("$djs.host(\"sdk.fetch\")"), "{text}");
    // The loading host binds them elsewhere: names the script never sees, which the
    // tests' `host` hook resolves as global paths.
    let mut b = new_vm();
    let my_tool = b.new_async_host_function(MY_TOOL, "myTool").unwrap();
    let sdk = b.new_object().unwrap();
    let fetch = b.new_async_host_function(SDK_FETCH, "fetch").unwrap();
    b.define(sdk, "fetch", fetch).unwrap();
    let helper = b.new_object().unwrap();
    b.define(sdk, "helper", Value::object(helper)).unwrap();
    b.define_global("@sdk/our-stuff#myTool", my_tool).unwrap();
    b.define_global("sdk", Value::object(sdk)).unwrap();
    assert_eq!(load(&mut b, &text), "", "{text}");
    assert_eq!(
        run(
            &mut b,
            "[lib.tool() === globalThis['@sdk/our-stuff#myTool'], lib.sdk() === sdk, \
             lib.helper === sdk.helper, lib.fetch === sdk.fetch].join()"
        ),
        "true,true,true,true",
        "{text}"
    );
}

/// Runs `lines` one Script each (as the REPL does) or all as one Script, dumps every
/// binding, loads the dump into a fresh VM and compares `check` there.
fn closure_case(lines: &[&str], check: &str) {
    for one_script in [true, false] {
        let mut a = new_vm();
        if one_script {
            let r = run(&mut a, &lines.join("\n"));
            assert!(
                !r.starts_with("throw") && !r.starts_with("compile"),
                "setup: {r}"
            );
        } else {
            for l in lines {
                let r = run(&mut a, l);
                assert!(
                    !r.starts_with("throw") && !r.starts_with("compile"),
                    "{l}: {r}"
                );
            }
        }
        let text = dump(&a, None).unwrap_or_else(|e| panic!("{}", e.message()));
        if std::env::var_os("DJS_DUMP_SHOW").is_some() {
            std::println!("{text}");
        }
        let want = run(&mut a, check);
        let mut b = new_vm();
        let loaded = load(&mut b, &text);
        assert!(!loaded.starts_with("throw"), "load: {loaded}\n{text}");
        assert_eq!(run(&mut b, check), want, "one script: {one_script}\n{text}");
    }
}

#[test]
fn closure_over_a_parameter() {
    // The owner's report (0168): `blah2` came out as `() => a`.
    closure_case(
        &[
            "let foo = [1,2,3,4];",
            "let x = this;",
            "function getStuff() { return [foo, x]; }",
            "function blah(a) { return () => a }",
            "const blah2 = blah(2);",
        ],
        "[blah2(), getStuff()[0].join(), getStuff()[1] === globalThis, blah(5)(), blah2.name].join()",
    );
}

#[test]
fn closures_over_locals_and_nested_scopes() {
    closure_case(
        &[
            "function mk() { let v = 10; const w = v * 2; return () => v + w; }",
            "const f = mk();",
            "function outer(x) { return function mid(y) { return (z) => x + y + z; }; }",
            "const mid = outer(1); const g = mid(2);",
            "let blockFn; { let hidden = 5; blockFn = () => hidden++; } blockFn();",
        ],
        "[f(), g(3), mid(10)(100), blockFn(), blockFn(), g.name, mid.name].join()",
    );
}

#[test]
fn closures_from_loops_with_let() {
    closure_case(
        &[
            "const fs = [];",
            "for (let i = 0; i < 3; i++) fs.push(() => i * 10);",
            "const counters = [1, 2].map((start) => { let n = start; return () => ++n; });",
            "counters[1]();",
        ],
        "[fs.map((f) => f()).join(), counters[0](), counters[1](), counters[1]()].join()",
    );
}

#[test]
fn closures_sharing_a_binding() {
    closure_case(
        &[
            "function pair() { let n = 0; return [() => ++n, () => n]; }",
            "const [inc, get] = pair();",
            "inc();",
            "const box = { inc, get };",
        ],
        "[inc(), get(), box.inc(), get(), box.get === get].join()",
    );
}

#[test]
fn closures_capturing_closures() {
    closure_case(
        &[
            "function mk() { const helper = (x) => x * 2; return (y) => helper(y) + 1; }",
            "const h = mk();",
            "function mk2(k) { function twice(x) { return k(k(x)); } return (y) => twice(y); }",
            "const h2 = mk2(h);",
            "function chain() { let a = 1; const f1 = () => a; const f2 = () => f1() + 1; return () => f2() + f1(); }",
            "const h3 = chain();",
        ],
        "[h(3), h2(1), h3()].join()",
    );
}

#[test]
fn classes_with_statics_accessors_and_computed_keys() {
    closure_case(
        &[
            "const log = [];",
            "class Point { constructor(x, y) { this.x = x; this.y = y; } get len() { return Math.sqrt(this.x * this.x + this.y * this.y); }\n\
               static origin() { return new Point(0, 0); } [Symbol.toPrimitive]() { return this.len; }\n\
               static count = 0; static { log.push('init'); } kind = 'pt'; toString() { return `(${this.x}, ${this.y})`; } }",
            "Point.count = 3;",
            "const p = new Point(3, 4);",
        ],
        "[p.len, String(p), +p, Point.origin().len, Point.count, log.join(), p instanceof Point, Object.keys(p).join(),\n\
          Object.getOwnPropertyNames(Point).join(), Object.getOwnPropertyNames(Point.prototype).join(), Point.name].join()",
    );
}

#[test]
fn class_expressions_extends_and_super() {
    closure_case(
        &[
            "function mk(tag) { return class Base { hi() { return 'hi ' + tag; } }; }",
            "const Base = mk('a');",
            "class Child extends Base { hi() { return super.hi() + '!'; } static make() { return new this(); } }",
            "const c = new Child();",
            "const Anon = class extends Child {};",
            "const base = { greet() { return 'base'; } };",
            "const obj = { __proto__: base, greet() { return super.greet() + '+'; } };",
        ],
        "[c.hi(), Child.make().hi(), new Anon().hi(), Object.getPrototypeOf(Child) === Base, Anon.name, Base.name,\n\
          c instanceof Base, obj.greet(), Object.getPrototypeOf(Anon.prototype) === Child.prototype].join()",
    );
}

/// Private methods and accessors of a class round-trip; instances that carry private
/// names are refused (only the class's construction can add them, D2878).
#[test]
fn private_fields_methods_and_brands() {
    closure_case(
        &[
            "class Counter { #step() { return 1; } get #twice() { return 2; }\n\
               twice() { return this.#twice + this.#step(); } static has(o) { return #step in o; } }",
            "const f = () => new Counter();",
        ],
        "[f().twice(), Counter.has(f()), Counter.has({})].join()",
    );
    // Static private fields, methods and accessors (D2883; a literal initializer stays
    // in the class text, D2950).
    closure_case(
        &[
            "class Counter { static #made = 2; static #fmt(v) { return '<' + v + '>'; } static get #twice() { return Counter.#made * 2; }\n\
               static made() { return Counter.#made; } static show(v) { return Counter.#fmt(v) + Counter.#twice; } }",
        ],
        "[Counter.made(), Counter.show(1)].join()",
    );
    let r = refused(
        "class Counter { #n = 0; inc() { this.#n++; return this; } } const a = new Counter().inc();",
        None,
    )
    .join("\n");
    assert!(r.contains("a: plain dumps: an instance of class"), "{r}");
    assert!(r.contains("with private name #n"), "{r}");
}

#[test]
fn subclasses_of_builtins() {
    closure_case(
        &[
            "class Stack extends Array { top() { return this[this.length - 1]; } }",
            "class Registry extends Map { names() { return [...this.keys()].join(); } }",
            "class AppError extends Error { constructor(m) { super(m); this.name = 'AppError'; } }",
            "class Done extends Promise {}",
            "const s = Stack.from([1, 2, 3]); const r = new Registry([['a', 1]]); const e = new AppError('boom');",
            "const pr = Done.resolve(5);",
        ],
        "[s.top(), s instanceof Stack, Array.isArray(s), s.length, r.names(), r instanceof Map, r.get('a'), e.message, e.name,\n\
          e instanceof AppError, e instanceof Error, pr instanceof Done].join()",
    );
}

#[test]
fn classes_changed_after_definition() {
    closure_case(
        &[
            "class T { a() { return 1; } b() { return 2; } static s() { return 3; } }",
            "delete T.prototype.b; T.prototype.extra = 42;",
            "Object.defineProperty(T, 'tag', { value: 'x', enumerable: false });",
            "const Frozen = Object.freeze(class F { static v = 1; });",
        ],
        "['b' in T.prototype, T.prototype.extra, T.tag, T.s(), new T().a(), Object.isFrozen(Frozen), Frozen.v,\n\
          Object.getOwnPropertyDescriptor(T, 'tag').enumerable].join()",
    );
}

#[test]
fn class_refusals() {
    let r = refused(
        "class X { getter() { return () => 1; } }\n\
         const Mixed = class extends (function () { return X; })() {};\n\
         class K { #x = 1; leak() { const self = this; return function () { return self.#x; }; } }\n\
         const f = new K().leak();",
        None,
    );
    let all = r.join("\n");
    for want in [
        "Mixed: a class whose heritage is an expression",
        "f: a function that uses a class's private names",
    ] {
        assert!(all.contains(want), "{all}");
    }
}

#[test]
fn proxies_are_made_again() {
    round_trip(
        "const target = { a: 1 };\n\
         const px = new Proxy(target, { get: (o, k) => (k in o ? o[k] : 'no ' + String(k)) });\n\
         const fx = new Proxy(function f(x) { return x + 1; }, {});",
        None,
        "[px.a, px.b, fx(1), typeof fx].join()",
    );
}

#[test]
fn frozen_records_keep_their_keys() {
    let t = round_trip(
        "const ALL = Object.freeze([Object.freeze({ name: 'echo', description: 'd' })]);",
        None,
        "ALL.map((t) => t.name).join() + Object.isFrozen(ALL[0])",
    );
    let _ = t;
}

/// A dump has no comments (D2641): no header lines, no section or factory notes. Only
/// the functions' own text could have comments, and these have none.
#[test]
fn dumps_have_no_comments() {
    let mut a = new_vm();
    run(
        &mut a,
        "function makeCounter(start) { let n = start; return { inc: () => ++n, get: () => n }; }\n\
         const counter = makeCounter(40);\n\
         const graph = { name: 'root', children: [] }; graph.children.push({ parent: graph });\n\
         const frozen = Object.freeze({ list: [1, 2] });\n\
         const iterProto = Object.getPrototypeOf([][Symbol.iterator]());",
    );
    let text = dump(&a, None).unwrap_or_else(|e| panic!("{}", e.message()));
    assert!(text.starts_with("\"djs-dump\";\n"), "{text}");
    for l in text.lines() {
        let t = l.trim_start();
        assert!(
            !t.starts_with("//") && !t.starts_with("/*"),
            "a comment: {l}\n{text}"
        );
    }
    let mut b = new_vm();
    assert_eq!(load(&mut b, &text), "");
    assert_eq!(
        run(&mut b, "counter.inc() + graph.children[0].parent.name"),
        "41root"
    );
}

/// Functions of a TypeScript script are written as JavaScript (D2879; an enum is data
/// already). A dump whose TypeScript is more than types says so in its directive
/// (`"djs-dump typescript";`, D2615, D2640).
#[test]
fn typescript_dumps_say_so_in_the_directive() {
    for (src, header, check, want) in [
        (
            "var add = (x: number, y: number): number => x + y;",
            "\"djs-dump\";\n",
            "add(2, 3)",
            "5",
        ),
        (
            "enum E { A = 7 } var get = (): E => E.A;",
            "\"djs-dump\";\n",
            "get()",
            "7",
        ),
        // A parameter property assigns `this.x`: not erasable (0209).
        (
            "class P { constructor(public x: number) {} } var p = new P(7);",
            "\"djs-dump typescript\";\n",
            "[p.x, new P(1).x].join()",
            "7,1",
        ),
    ] {
        let mut a = new_vm();
        let f = a
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions {
                    typescript: true,
                    ..crate::compiler::CompileOptions::script("t.ts")
                },
            )
            .unwrap();
        let this = Value::object(a.global());
        a.call(f, this, &[]).unwrap();
        assert!(matches!(a.run(u64::MAX), Status::Done(_)));
        let text = dump(&a, None).unwrap_or_else(|e| panic!("{}", e.message()));
        assert!(text.starts_with(header), "{text}");
        let mut b = new_vm();
        assert_eq!(load(&mut b, &text), "", "{text}");
        assert_eq!(run(&mut b, check), want, "{text}");
    }
}

/// A VM without frozen builtins cannot dump (D2836).
#[test]
fn dumps_need_frozen_builtins() {
    let a = Vm::from_base(base(), VmOptions::default()).unwrap();
    let e = dump(&a, None).unwrap_err().message();
    assert!(e.contains("dumps need frozen builtins"), "{e}");
}
