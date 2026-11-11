//! Dump coverage on the harvested codemode corpus (0167, docs/functions-as-code.md
//! "Coverage"): every case of tests/corpus/codemode-harvest runs as the difftest runs it
//! (the corpus prelude, fake tools), then the top-level bindings of the script's body are
//! dumped with `Vm::dump_values` and the dump is loaded into a fresh VM.
//!
//! `DJS_HARVEST=1 cargo test -p djs-core --lib vm::dump_harvest -- --ignored --nocapture`
//! (`DJS_HARVEST_OUT=file` writes one line per case).

extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::collections::BTreeMap;

use super::dump::DumpOptions;
use super::*;
use crate::bytecode::template;
use crate::compiler::CompileOptions;
use crate::heap::Atom;
use crate::heap::cells::{FuncTemplate, U32Array};

pub(super) fn base() -> &'static [u8] {
    static BASE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        let mut v = Vm::new(VmOptions::default()).unwrap();
        v.prepare_base_image().unwrap();
        v.bytes().to_vec()
    })
}

/// The base's builtins and paths, as djs-prelude's `BASE_NAMES` has them for its base
/// (`DumpOptions::base_names`): made once.
pub(super) fn base_names() -> super::dump::BaseNames<'static> {
    type Names = (Vec<(u32, &'static str)>, Vec<(u32, &'static str, u64, u8)>);
    static NAMES: std::sync::OnceLock<Names> = std::sync::OnceLock::new();
    let leak = |s: String| &*alloc::boxed::Box::leak(s.into_boxed_str());
    let (b, g) = NAMES.get_or_init(|| {
        let h = crate::heap::Heap::from_bytes_trusted(base().to_vec(), usize::MAX).unwrap();
        let end = h.base_end();
        let mut builtins: Vec<(u32, &'static str)> = super::registry::reachable_names(&h)
            .into_iter()
            .filter(|(c, _)| *c < end)
            .map(|(c, p)| (c, leak(p)))
            .collect();
        builtins.sort_by_key(|b| b.0);
        let mut globals: Vec<(u32, &'static str, u64, u8)> = super::registry::base_globals(&h)
            .into_iter()
            .map(|(a, n, b, f)| (a, leak(n), b, f))
            .collect();
        globals.sort_by_key(|g| g.0);
        (builtins, globals)
    });
    super::dump::BaseNames {
        builtins: b,
        globals: g,
    }
}

/// The VMs' options: frozen builtins, which dumps need (D2836).
/// The VMs' options; with `DJS_LAZY=1` lazy functions (D2990), as the dump module runs.
pub(super) fn frozen() -> VmOptions {
    extern crate std;
    VmOptions {
        frozen_builtins: true,
        #[cfg(feature = "lazy")]
        lazy_functions: std::env::var("DJS_LAZY").is_ok_and(|v| v == "1"),
        ..VmOptions::default()
    }
}

const OUTPUT: u32 = 0;
const DONE: u32 = 1;
/// The globals the corpus prelude defines (pi's codemode globals).
const HOST: &[&str] = &[
    "console",
    "text",
    "exit",
    "tools",
    "ALL_TOOLS",
    "store",
    "load",
];

/// Calls `f` and runs it and its jobs to the end, answering the prelude's host calls.
fn call(vm: &mut Vm, f: Value, this: Value, args: &[Value]) -> Result<Value, String> {
    vm.call(f, this, args).map_err(|e| format!("{e:?}"))?;
    let mut fuel = 0u64;
    loop {
        match vm.run(10_000_000) {
            Status::Done(v) => return Ok(v),
            Status::Threw(e) => return Err(format!("threw {}", vm.display(e))),
            Status::OutOfFuel => {
                fuel += 1;
                if fuel > 50 {
                    return Err("out of fuel".into());
                }
            }
            Status::HostCall { call_id, .. } => {
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED))
                    .map_err(|e| format!("{e:?}"))?;
            }
            s => return Err(format!("{s:?}")),
        }
    }
}

fn get(vm: &mut Vm, o: Obj, name: &str) -> Value {
    let k = vm.heap_mut().atom_str(name).unwrap();
    vm.heap()
        .find_property(o, k)
        .and_then(|(_, p)| p.value())
        .unwrap_or(Value::UNDEFINED)
}

/// The names of the locals of the function whose source starts at byte `at` (in the
/// templates under `t`).
fn locals_at(h: &Heap, t: u32, at: u32) -> Option<Vec<String>> {
    let info = template::debug_info(h, t);
    if info.src_start == at {
        let a = h.arena();
        let locals = u32::from(a.get::<u16>(t, FuncTemplate::LOCALS));
        let names = info.names.get()?;
        let len: u32 = a.get(names, U32Array::LEN);
        return Some(
            (0..len.min(locals))
                .map(|i| h.atom_name(Atom::from_id(a.read::<u32>(U32Array::DATA.at(names, i)))))
                .collect(),
        );
    }
    (0..template::child_count(h, t))
        .filter_map(|i| template::child(h, t, i).and_then(|r| r.get()))
        .find_map(|c| locals_at(h, c, at))
}

#[derive(Default)]
struct Outcome {
    status: String,
    roots: usize,
    functions: usize,
    /// Classes among the bindings, and whether the script's text defines one.
    classes: usize,
    defines_class: bool,
    bytes: usize,
    refusals: Vec<String>,
    loaded: bool,
    /// Whether the loaded bindings print as the originals (JSON, functions as text).
    same: bool,
    /// Why the dump did not load: the error's class only.
    load_error: String,
}

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Whether a script's text has a class definition (`class Name`, `class {`, `class
/// extends`), outside of identifiers.
fn defines_class(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = 0;
    while let Some(at) = text.get(i..).and_then(|t| t.find("class")) {
        let p = i + at;
        i = p + 5;
        let before = p.checked_sub(1).map(|j| b[j]);
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'.')
        {
            continue;
        }
        let rest = text.get(p + 5..).unwrap_or("");
        if !rest.starts_with([' ', '\n', '\t', '{']) {
            continue;
        }
        let next = rest.trim_start();
        if next.starts_with('{')
            || next.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$')
        {
            return true;
        }
    }
    false
}

fn run_case(prelude: &str, case: &str) -> Outcome {
    let mut out = Outcome::default();
    // The script's own body: the arrow the harvest wraps it in, `return await (async
    // (tools, ...) => {` ... `})(fake, ...);`.
    let lines: Vec<&str> = case.lines().collect();
    let Some(open) = lines
        .iter()
        .position(|l| l.starts_with("return await (async ("))
    else {
        out.status = "no body".into();
        return out;
    };
    out.defines_class = defines_class(&lines[open..].join("\n"));
    let params: Vec<&str> = lines[open]
        .trim_start_matches("return await (async (")
        .split(')')
        .next()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .collect();
    let wrap = |c: &str| format!("(async (tools, console) => {{{c}\n}})");
    let original = wrap(case);
    let mut vm = Vm::from_base(base(), frozen()).unwrap();
    // The body's locals, from the compiled original.
    let offset = |src: &str| {
        let line_start: usize = src
            .lines()
            .take_while(|l| !l.starts_with("return await (async ("))
            .map(|l| l.len() + 1)
            .sum();
        (line_start + "return await (".len()) as u32
    };
    let names = match vm.compile(original.as_bytes(), &CompileOptions::script("case.js")) {
        Ok(f) => {
            let t = vm.heap().closure_template(f.as_object().unwrap());
            locals_at(vm.heap(), t, offset(&original)).unwrap_or_default()
        }
        Err(e) => {
            out.status = format!("compile: {}", e.message);
            return out;
        }
    };
    let names: Vec<String> = names
        .into_iter()
        .filter(|n| is_ident(n) && !params.contains(&n.as_str()) && n != "arguments")
        .collect();
    // A prologue that publishes the locals (read at the end, TDZ ones left out) and makes
    // the fake tools the global `tools`, as pi's are.
    let mut prologue =
        String::from("globalThis.tools = tools; globalThis.__snap = () => { const o = {};");
    for n in &names {
        prologue += &format!(" try {{ o.{n} = {n}; }} catch {{}}");
    }
    prologue += " return o; };";
    let mut src_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    src_lines.insert(open + 1, prologue);
    let src = wrap(&src_lines.join("\n"));

    // The difftest's run: prelude, host object, api.run(case), api.stalled().
    let run = (|| -> Result<(), String> {
        let p = vm
            .compile(prelude.as_bytes(), &CompileOptions::script("prelude.js"))
            .map_err(|e| e.message)?;
        let g = Value::object(vm.global());
        let setup = call(&mut vm, p, g, &[])?;
        let host = vm.new_object().map_err(|e| format!("{e:?}"))?;
        for (id, name) in [(OUTPUT, "output"), (DONE, "done")] {
            let f = vm
                .new_host_function(id, name)
                .map_err(|e| format!("{e:?}"))?;
            vm.define(host, name, f).map_err(|e| format!("{e:?}"))?;
        }
        let api = call(&mut vm, setup, Value::UNDEFINED, &[Value::object(host)])?;
        let api = vm
            .heap_mut()
            .new_handle(api)
            .map_err(|e| format!("{e:?}"))?;
        let script = vm
            .compile(src.as_bytes(), &CompileOptions::script("case.js"))
            .map_err(|e| e.message)?;
        let g = Value::object(vm.global());
        let f = call(&mut vm, script, g, &[])?;
        let apiv = vm.heap().get_handle(api);
        let run = get(&mut vm, apiv.as_object().unwrap(), "run");
        call(&mut vm, run, apiv, &[f])?;
        let apiv = vm.heap().get_handle(api);
        let stalled = get(&mut vm, apiv.as_object().unwrap(), "stalled");
        call(&mut vm, stalled, apiv, &[])?;
        Ok(())
    })();
    if let Err(e) = run {
        out.status = format!("run: {e}");
        return out;
    }
    // What both VMs must agree on: the bindings as JSON, functions as their text. First,
    // since running code moves objects (the Values below are not rooted).
    let fingerprint = |vm: &mut Vm, list: &str| -> Option<String> {
        let src = format!(
            "(() => {{ try {{ return JSON.stringify({list}.map((v) => typeof v === 'function' ? String(v) : v)); }} \
             catch {{ return 'unserializable'; }} }})()"
        );
        let f = vm
            .compile(src.as_bytes(), &CompileOptions::script("fp.js"))
            .ok()?;
        let g = Value::object(vm.global());
        let v = call(vm, f, g, &[]).ok()?;
        Some(vm.display(v))
    };
    let want = fingerprint(&mut vm, "Object.values(__snap())");
    let g = vm.global();
    let snap = get(&mut vm, g, "__snap");
    let Ok(locals) = call(&mut vm, snap, Value::UNDEFINED, &[]) else {
        out.status = "no snapshot".into();
        return out;
    };
    let lo = locals.as_object().unwrap();
    let mut roots: Vec<(String, Value)> = Vec::new();
    for k in vm.heap().own_keys(lo) {
        let name = vm.heap().atom_name(k);
        let v = vm
            .heap()
            .get_own_property(lo, k)
            .and_then(|p| p.value())
            .unwrap();
        if v.as_object().is_some_and(|o| {
            matches!(
                vm.heap().object_class(o),
                crate::heap::ObjectClass::Function
            )
        }) {
            out.functions += 1;
            if super::dump_classes::is_class(vm.heap(), v.as_object().unwrap()) {
                out.classes += 1;
            }
        }
        roots.push((name, v));
    }
    out.roots = roots.len();
    let list = format!(
        "[{}]",
        roots
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let refs: Vec<(&str, Value)> = roots.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    match vm.dump_values(
        &DumpOptions {
            base: base(),
            host: HOST,
            ..Default::default()
        },
        &refs,
    ) {
        Ok(text) => {
            out.bytes = text.len();
            out.status = "dumped".into();
            // Loads in a fresh VM with stand-ins for the host's globals.
            let mut b = Vm::from_base(base(), frozen()).unwrap();
            let stubs = "globalThis.console = { log() {} }; globalThis.text = () => {}; \
                         globalThis.tools = {}; globalThis.store = () => {}; globalThis.load = () => {}; \
                         globalThis.exit = () => {}; globalThis.ALL_TOOLS = [];";
            out.loaded = true;
            for (i, s) in [stubs, text.as_str()].into_iter().enumerate() {
                let r = if i == 1 {
                    let none = |_: &str| None;
                    let mut serve = |vm: &mut Vm, id: u32, _: u32| {
                        vm.resolve_host_call(id, Ok(Value::UNDEFINED)).unwrap();
                    };
                    super::dump_tests::run_plain_dump(&mut b, s, &none, &mut serve).map(|_| ())
                } else {
                    match b.compile(s.as_bytes(), &CompileOptions::script("dump.js")) {
                        Ok(f) => {
                            let g = Value::object(b.global());
                            call(&mut b, f, g, &[]).map(|_| ())
                        }
                        Err(e) => Err(format!("compile {:?}", e.kind)),
                    }
                };
                if let Err(e) = r {
                    out.loaded = false;
                    let e = e.trim_start_matches("threw ");
                    out.load_error = e.split([':', ' ']).next().unwrap_or("").to_string();
                    // Which kind of name is missing (counts only for private inputs).
                    if let Some(id) = e.split_whitespace().nth(1) {
                        let kind = if names.iter().any(|n| n == id) {
                            "a body local"
                        } else if params.contains(&id) {
                            "a body parameter"
                        } else {
                            "another name"
                        };
                        let defined = text.contains(&format!("{id} = "))
                            || text.contains(&format!("function {id}"));
                        out.load_error += &format!(" ({kind}, defined in the dump: {defined})");
                    }
                }
            }
            if out.loaded {
                out.same = want.is_some() && fingerprint(&mut b, &list) == want;
                if !out.same && std::env::var_os("DJS_HARVEST_CLASSIFY").is_some() {
                    let got = fingerprint(&mut b, &list);
                    let class = |s: &Option<String>| match s.as_deref() {
                        None => "none",
                        Some("unserializable") => "unserializable",
                        Some(_) => "json",
                    };
                    let w = want.clone().unwrap_or_default();
                    let g = got.clone().unwrap_or_default();
                    let common = w.bytes().zip(g.bytes()).take_while(|(a, b)| a == b).count();
                    // The kind of character where they part (no content).
                    let at = |s: &str| {
                        s.as_bytes().get(common).map_or('$', |c| {
                            if c.is_ascii_digit() {
                                '9'
                            } else if c.is_ascii_alphabetic() {
                                'a'
                            } else {
                                *c as char
                            }
                        })
                    };
                    std::println!(
                        "differs: want {} got {}, lengths {} {}, part at {} ({:?} vs {:?})",
                        class(&want),
                        class(&got),
                        w.len(),
                        g.len(),
                        common,
                        at(&w),
                        at(&g)
                    );
                }
            }
        }
        Err(e) => {
            out.status = "refused".into();
            out.refusals = e.0;
        }
    }
    out
}

/// Runs `cases` (name, case text) and prints the summary; per-case lines to
/// `DJS_HARVEST_OUT` (refusal paths included: not for private inputs).
fn coverage(cases: impl Iterator<Item = (String, String)>, private: bool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let prelude = std::fs::read_to_string(root.join("prelude.js")).unwrap();
    let mut lines = String::new();
    let mut status: BTreeMap<String, usize> = BTreeMap::new();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut load_errors: BTreeMap<String, usize> = BTreeMap::new();
    let (mut n, mut with_roots, mut with_fns, mut dumped, mut loaded, mut fns, mut fns_dumped) =
        (0, 0, 0, 0, 0, 0, 0);
    let mut same = 0;
    let (mut def_class, mut with_class, mut classes, mut class_dumped) = (0, 0, 0, 0);
    let mut def_class_ok = 0;
    let mut bytes = 0;
    for (name, case) in cases {
        n += 1;
        let o = run_case(&prelude, &case);
        let st = o.status.split(':').next().unwrap_or("").to_string();
        *status.entry(st).or_default() += 1;
        if o.roots > 0 {
            with_roots += 1;
        }
        if o.functions > 0 {
            with_fns += 1;
        }
        fns += o.functions;
        def_class += usize::from(o.defines_class);
        def_class_ok += usize::from(o.defines_class && o.status == "dumped" && o.same);
        with_class += usize::from(o.classes > 0);
        classes += o.classes;
        class_dumped += usize::from(o.classes > 0 && o.status == "dumped" && o.same);
        if o.status == "dumped" {
            dumped += 1;
            fns_dumped += o.functions;
            bytes += o.bytes;
            same += usize::from(o.same);
            if o.loaded {
                loaded += 1;
            } else {
                *load_errors.entry(o.load_error.clone()).or_default() += 1;
            }
        }
        let mut seen = Vec::new();
        for r in &o.refusals {
            let why = r.split_once(": ").map_or(r.as_str(), |(_, w)| w);
            let why = why.split(" (").next().unwrap_or(why).to_string();
            if !seen.contains(&why) {
                *reasons.entry(why.clone()).or_default() += 1;
                seen.push(why);
            }
        }
        if !private {
            lines += &format!(
                "{name}\t{}\troots={}\tfns={}\tbytes={}\tloaded={}\tsame={}\t{}\n",
                o.status,
                o.roots,
                o.functions,
                o.bytes,
                o.loaded,
                o.same,
                o.refusals.join(" | ")
            );
        }
    }
    if let Some(p) = std::env::var_os("DJS_HARVEST_OUT")
        && !private
    {
        std::fs::write(p, &lines).unwrap();
    }
    std::println!("cases: {n}");
    std::println!("status: {status:?}");
    std::println!(
        "with top-level bindings: {with_roots}; with function bindings: {with_fns} ({fns} functions)"
    );
    std::println!(
        "dumped: {dumped} (functions {fns_dumped}), loaded: {loaded}, same bindings: {same}, dump bytes: {bytes}"
    );
    std::println!(
        "defining a class: {def_class} ({def_class_ok} dumped and equal); with class bindings: {with_class} ({classes} classes), dumped and equal: {class_dumped}"
    );
    std::println!("refusal reasons (cases): {reasons:?}");
    std::println!("load errors: {load_errors:?}");
}

#[test]
#[ignore]
fn harvest_coverage() {
    if std::env::var_os("DJS_HARVEST").is_none() {
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let mut files: Vec<_> = std::fs::read_dir(root.join("codemode-harvest"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "js"))
        .collect();
    files.sort();
    coverage(
        files.into_iter().map(|f| {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&f).unwrap())
        }),
        false,
    );
}

/// The same over private cases in harvest form (`DJS_EXTRACT_DIR=dir` of `.js` files, made
/// from an extraction of pi codemode scripts by a script outside the repository): prints
/// counts only, never names or paths.
#[test]
#[ignore]
fn extract_coverage() {
    let Some(dir) = std::env::var_os("DJS_EXTRACT_DIR") else {
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    files.sort();
    coverage(
        files
            .into_iter()
            .map(|f| (String::new(), std::fs::read_to_string(&f).unwrap())),
        true,
    );
}

// ---- Continuations (0172): a dump at every idle point ---------------------------------

const LATER: u32 = 2;

#[derive(Default)]
struct IdleStats {
    cases: usize,
    with_idle: usize,
    idle: usize,
    dumped: usize,
    loaded: usize,
    same: usize,
    differ: usize,
    /// Idle points by the first refusal reason.
    refusals: BTreeMap<String, usize>,
    load_errors: BTreeMap<String, usize>,
}

pub(super) fn host_fn_name(id: u32) -> Option<String> {
    match id {
        OUTPUT => Some("output".into()),
        DONE => Some("done".into()),
        LATER => Some("later".into()),
        _ => None,
    }
}

/// A fresh VM to run a dump of the idle run in: the host's functions as globals, by the
/// names the dump gives them (`host_fn_name`).
pub(super) fn load_vm() -> Vm {
    let mut b = Vm::from_base(base(), frozen()).unwrap();
    for (id, name) in [(OUTPUT, "output"), (DONE, "done")] {
        let f = b.new_host_function(id, name).unwrap();
        b.define_global(name, f).unwrap();
    }
    let f = b.new_async_host_function(LATER, "later").unwrap();
    b.define_global("later", f).unwrap();
    b
}

pub(super) fn host_fn(name: &str) -> Option<u32> {
    match name {
        "output" => Some(OUTPUT),
        "done" => Some(DONE),
        "later" => Some(LATER),
        _ => None,
    }
}

/// What `idle_call` does at an idle point besides settling the oldest `later` call.
enum AtIdle<'a> {
    Nothing,
    /// Dump, load into a fresh VM and go on there.
    Dump(&'a mut IdleStats),
    /// Call the hook with the VM and the prelude's api (dump_perf.rs).
    Hook(&'a mut dyn FnMut(&Vm, Value)),
}

/// Runs `f` to the end as the difftest's djs engine does: output and done recorded,
/// `later` calls settled at idle points, oldest first; with `dump`, every idle point is
/// dumped and the run continues in a fresh VM that loaded the dump (`api` moves along).
fn idle_call(
    vm: &mut Vm,
    api: &mut crate::heap::Handle,
    f: Value,
    this: Value,
    args: &[Value],
    out: &mut Vec<String>,
    at_idle: AtIdle<'_>,
) -> Result<Value, String> {
    let (mut dump, mut hook) = match at_idle {
        AtIdle::Nothing => (None, None),
        AtIdle::Dump(st) => (Some(st), None),
        AtIdle::Hook(h) => (None, Some(h)),
    };
    vm.call(f, this, args).map_err(|e| format!("{e:?}"))?;
    let mut slices = 0;
    loop {
        match vm.run(10_000_000) {
            Status::Done(v) => return Ok(v),
            Status::Threw(e) => return Err(format!("threw {}", vm.display(e))),
            Status::OutOfFuel => {
                slices += 1;
                if slices > 100 {
                    return Err("out of fuel".into());
                }
            }
            Status::HostCall { call_id, fn_id } => {
                let info = vm.host_call(call_id).ok_or("no call")?;
                let s = |v: Value| {
                    v.as_string()
                        .map(|t| vm.heap().str_to_string(t))
                        .unwrap_or_default()
                };
                match fn_id {
                    OUTPUT => out.push(s(info.args[0])),
                    DONE => out.push(format!("=== {} {}", s(info.args[0]), s(info.args[1]))),
                    _ => return Err("unknown host function".into()),
                }
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED))
                    .map_err(|e| format!("{e:?}"))?;
            }
            Status::Idle => {
                if let Some(hook) = hook.as_deref_mut() {
                    hook(vm, vm.heap().get_handle(*api));
                }
                if let Some(st) = dump.as_deref_mut() {
                    st.idle += 1;
                    let apiv = vm.heap().get_handle(*api);
                    // The api is the dump's completion value (D2895).
                    let handles = [("run", apiv)];
                    let opts = DumpOptions {
                        base: base(),
                        host_fn_name: &host_fn_name,
                        handles: &handles,
                        ..Default::default()
                    };
                    match vm.dump(&opts, None) {
                        Err(e) => {
                            let first = e.0.first().cloned().unwrap_or_default();
                            let why = first.split_once(": ").map_or(first.as_str(), |(_, w)| w);
                            let why = why.to_string();
                            *st.refusals.entry(why).or_default() += 1;
                        }
                        Ok(text) => {
                            st.dumped += 1;
                            let mut b = load_vm();
                            let mut serve = |vm: &mut Vm, id: u32, _: u32| {
                                vm.resolve_host_call(id, Ok(Value::UNDEFINED)).unwrap();
                            };
                            match super::dump_tests::run_plain_dump(
                                &mut b, &text, &host_fn, &mut serve,
                            ) {
                                Ok(v) => {
                                    st.loaded += 1;
                                    *api = b.heap_mut().new_handle(v).unwrap();
                                    *vm = b;
                                }
                                Err(m) => {
                                    let m =
                                        m.split([':', '\n']).take(2).collect::<Vec<_>>().join(":");
                                    *st.load_errors.entry(m).or_default() += 1;
                                }
                            }
                        }
                    }
                }
                let Some(&id) = vm.pending_host_calls().iter().min() else {
                    return Err("idle with nothing pending".into());
                };
                let info = vm.host_call(id).ok_or("no call")?;
                let v = info.args.first().copied().unwrap_or(Value::UNDEFINED);
                let rejected = info.args.get(1) == Some(&Value::TRUE);
                vm.resolve_host_call(id, if rejected { Err(v) } else { Ok(v) })
                    .map_err(|e| format!("{e:?}"))?;
            }
            s => return Err(format!("{s:?}")),
        }
    }
}

/// A case's transcript as the difftest's djs engine makes it; with `dump`, through a dump
/// at every idle point.
fn idle_run(
    prelude: &str,
    case: &str,
    dump: Option<&mut IdleStats>,
) -> Result<Vec<String>, String> {
    idle_run_hooked(prelude, case, dump.map_or(AtIdle::Nothing, AtIdle::Dump))
}

/// [`idle_run`] calling `hook` with the VM and the prelude's api at every idle point
/// (dump_perf.rs).
pub(super) fn idle_run_with(
    prelude: &str,
    case: &str,
    hook: &mut dyn FnMut(&Vm, Value),
) -> Result<Vec<String>, String> {
    idle_run_hooked(prelude, case, AtIdle::Hook(hook))
}

fn idle_run_hooked(prelude: &str, case: &str, at_idle: AtIdle<'_>) -> Result<Vec<String>, String> {
    let mut vm = Vm::from_base(base(), frozen()).unwrap();
    let mut out = Vec::new();
    let p = vm
        .compile(prelude.as_bytes(), &CompileOptions::script("prelude.js"))
        .map_err(|e| e.message)?;
    let g = Value::object(vm.global());
    let mut none = vm.heap_mut().new_handle(Value::UNDEFINED).unwrap();
    let setup = idle_call(&mut vm, &mut none, p, g, &[], &mut out, AtIdle::Nothing)?;
    let host = vm.new_object().map_err(|e| format!("{e:?}"))?;
    for (id, name) in [(OUTPUT, "output"), (DONE, "done"), (LATER, "later")] {
        let f = if id == LATER {
            vm.new_async_host_function(id, name)
        } else {
            vm.new_host_function(id, name)
        }
        .map_err(|e| format!("{e:?}"))?;
        vm.define(host, name, f).map_err(|e| format!("{e:?}"))?;
    }
    let apiv = idle_call(
        &mut vm,
        &mut none,
        setup,
        Value::UNDEFINED,
        &[Value::object(host)],
        &mut out,
        AtIdle::Nothing,
    )?;
    let mut api = vm
        .heap_mut()
        .new_handle(apiv)
        .map_err(|e| format!("{e:?}"))?;
    let src = format!("(async (tools, console) => {{{case}\n}})");
    let script = vm
        .compile(src.as_bytes(), &CompileOptions::script("case.js"))
        .map_err(|e| e.message)?;
    let g = Value::object(vm.global());
    let f = idle_call(&mut vm, &mut api, script, g, &[], &mut out, AtIdle::Nothing)?;
    let apiv = vm.heap().get_handle(api);
    let run = get(&mut vm, apiv.as_object().unwrap(), "run");
    idle_call(&mut vm, &mut api, run, apiv, &[f], &mut out, at_idle)?;
    let apiv = vm.heap().get_handle(api);
    let stalled = get(&mut vm, apiv.as_object().unwrap(), "stalled");
    idle_call(
        &mut vm,
        &mut api,
        stalled,
        apiv,
        &[],
        &mut out,
        AtIdle::Nothing,
    )?;
    // Stack frames name the dump's script after a load: compared without them, as the
    // difftest does.
    Ok(out
        .into_iter()
        .map(|o| {
            o.lines()
                .filter(|l| !l.trim_start().starts_with("at "))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect())
}

/// Counts over `cases`: idle points, dumps, loads, transcripts equal to the plain run's,
/// refusal reasons by idle point.
fn idle_coverage(cases: impl Iterator<Item = String>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let prelude = std::fs::read_to_string(root.join("prelude.js")).unwrap();
    let mut st = IdleStats::default();
    for case in cases {
        st.cases += 1;
        let Ok(want) = idle_run(&prelude, &case, None) else {
            continue;
        };
        let before = st.idle;
        let got = idle_run(&prelude, &case, Some(&mut st));
        if st.idle > before {
            st.with_idle += 1;
            if got.as_ref() == Ok(&want) {
                st.same += 1;
            } else {
                st.differ += 1;
            }
        }
    }
    std::println!(
        "cases: {}, with idle points: {}, idle points: {}, dumped: {}, loaded: {}",
        st.cases,
        st.with_idle,
        st.idle,
        st.dumped,
        st.loaded
    );
    std::println!(
        "cases with idle points whose transcript equals the plain run's: {}, differ: {}",
        st.same,
        st.differ
    );
    std::println!("refusals (idle points, first reason): {:?}", st.refusals);
    std::println!("load errors: {:?}", st.load_errors);
}

/// `DJS_HARVEST=1 cargo test -p djs-core --lib vm::dump_harvest::idle -- --ignored --nocapture`
#[test]
#[ignore]
fn idle_harvest_coverage() {
    if std::env::var_os("DJS_HARVEST").is_none() {
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let mut files: Vec<_> = std::fs::read_dir(root.join("codemode-harvest"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "js"))
        .collect();
    files.sort();
    idle_coverage(
        files
            .into_iter()
            .map(|f| std::fs::read_to_string(&f).unwrap()),
    );
}

/// The same over private cases (`DJS_EXTRACT_DIR=dir`, made by `scripts/harvest/build.py`
/// so that their fakes answer through `later`): counts only.
#[test]
#[ignore]
fn idle_extract_coverage() {
    let Some(dir) = std::env::var_os("DJS_EXTRACT_DIR") else {
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "js"))
        .collect();
    files.sort();
    idle_coverage(
        files
            .into_iter()
            .map(|f| std::fs::read_to_string(&f).unwrap()),
    );
}
