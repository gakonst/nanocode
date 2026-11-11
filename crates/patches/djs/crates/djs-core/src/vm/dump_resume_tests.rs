//! Dumps (D2830) of programs that wait: tests/dump/v2/ (programs `NAME.src.js` and the
//! golden programs with their dumps, golden/), the exporter's refusals and reports, and
//! plain dumps run as Scripts at every idle point and in chains.
//!
//! The programs print through the sync host function `print` and wait through the async
//! host function `later(v, rejected)`, which the driver settles at idle points, oldest
//! call first, with `v` (a rejection if `rejected`). A program's output up to idle point K
//! followed by the output of its dump's run must equal the uninterrupted run's. Dumps need
//! frozen builtins (D2836): the VMs that dump have them.

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

use super::*;

const LATER: u32 = 1;
const PRINT: u32 = 2;
/// A plain dump's `$djs.seed`.
const SEED: u32 = 3;

fn base() -> &'static [u8] {
    static BASE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        let mut v = Vm::new(VmOptions::default()).unwrap();
        v.prepare_base_image().unwrap();
        v.bytes().to_vec()
    })
}

/// A fresh VM with `later` and `print`.
fn new_vm() -> Vm {
    new_vm_with(false)
}

/// ... collecting at every safepoint with `stress`.
fn new_vm_with(stress: bool) -> Vm {
    let mut opts = VmOptions::default();
    opts.exec.gc_stress = stress;
    vm_with(opts)
}

/// A VM for plain dumps: frozen builtins, which the plain exporter requires (D2836).
fn plain_vm() -> Vm {
    vm_with(VmOptions {
        frozen_builtins: true,
        ..VmOptions::default()
    })
}

fn vm_with(opts: VmOptions) -> Vm {
    let mut vm = Vm::from_base(base(), opts).unwrap();
    let later = vm.new_async_host_function(LATER, "later").unwrap();
    vm.define_global("later", later).unwrap();
    let print = vm.new_host_function(PRINT, "print").unwrap();
    vm.define_global("print", print).unwrap();
    vm
}

fn host_fn(name: &str) -> Option<u32> {
    match name {
        "later" => Some(LATER),
        "print" => Some(PRINT),
        _ => None,
    }
}

fn host_fn_name(id: u32) -> Option<String> {
    match id {
        LATER => Some("later".into()),
        PRINT => Some("print".into()),
        _ => None,
    }
}

/// Runs the VM to its end, settling `later` calls at idle points (oldest first).
/// `marks[k]` is the number of lines printed when idle point k+1 was reached.
/// With `slice`, `run` gets that much fuel at a time.
fn drive(vm: &mut Vm, lines: &mut Vec<String>, marks: &mut Vec<usize>, slice: Option<u64>) {
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds < 1_000_000, "runaway");
        match vm.run(slice.unwrap_or(1_000_000_000)) {
            Status::OutOfFuel if slice.is_some() => {}
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, PRINT, "unexpected host call");
                let args = vm.host_call(call_id).unwrap().args;
                let parts: Vec<String> = args.iter().map(|&v| vm.display(v)).collect();
                lines.push(parts.join(" "));
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::Idle | Status::Done(_) => {
                let Some(&id) = vm.pending_host_calls().iter().min() else {
                    break;
                };
                marks.push(lines.len());
                settle(vm, id);
            }
            Status::Threw(e) => {
                let m = vm.display(e);
                lines.push(alloc::format!("threw {m}"));
                break;
            }
            s => panic!("{s:?}"),
        }
    }
    for r in vm.take_unhandled_rejections() {
        let m = vm.display(r);
        lines.push(alloc::format!("unhandled {m}"));
    }
}

/// Settles the `later` call `id` with its arguments.
fn settle(vm: &mut Vm, id: u32) {
    let info = vm.host_call(id).unwrap();
    assert_eq!(info.fn_id, LATER);
    let v = info.args.first().copied().unwrap_or(Value::UNDEFINED);
    let rejected = info.args.get(1).copied() == Some(Value::TRUE);
    vm.resolve_host_call(id, if rejected { Err(v) } else { Ok(v) })
        .unwrap();
}

/// The uninterrupted run of a program: its lines and idle marks.
fn run_program(src: &str) -> (Vec<String>, Vec<usize>) {
    let mut vm = new_vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("src.js"),
        )
        .unwrap_or_else(|e| panic!("{} at {}:{}", e.message, e.line, e.col));
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let (mut lines, mut marks) = (Vec::new(), Vec::new());
    drive(&mut vm, &mut lines, &mut marks, None);
    (lines, marks)
}

/// Runs a Script in an idle VM: its completion value.
fn run_value(vm: &mut Vm, src: &str) -> String {
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("check.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    match vm.run(1_000_000) {
        Status::Done(v) => vm.display(v),
        // (Idle: the dump's calls are open.)
        Status::Idle => match vm.outcome() {
            Some(Ok(v)) => vm.display(v),
            o => panic!("{o:?}"),
        },
        s => panic!("{s:?}"),
    }
}

fn fixture_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/dump/v2")
}

/// Runs `src` to idle point `k`, dumps it, runs the dump in a fresh VM to the end: the
/// lines before the dump and after, or why the dump was refused.
fn dump_at(src: &str, k: usize) -> Result<(Vec<String>, String), String> {
    let mut vm = plain_vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("src.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut lines = Vec::new();
    for _ in 1..k {
        let id = plain_step(&mut vm, &mut lines).expect("an idle point");
        settle(&mut vm, id);
    }
    plain_step(&mut vm, &mut lines).expect("an idle point");
    let text = plain_dump(&vm)?;
    if std::env::var("DJS_DUMP_SHOW").is_ok_and(|v| v == "all") {
        std::println!("{text}");
    }
    let mut vm = plain_load(&text)?;
    while let Some(id) = plain_step(&mut vm, &mut lines) {
        settle(&mut vm, id);
    }
    Ok((lines, text))
}

/// Runs dump `text` in `vm` (a fresh one, with the host's globals), the open calls
/// reopened under their ids (D2890), what it prints meanwhile in `lines`: the handle of
/// its completion value, the run's promise (D2895).
fn plain_run(
    vm: &mut Vm,
    text: &str,
    lines: &mut Vec<String>,
) -> Result<crate::heap::Handle, String> {
    let mut print = |vm: &mut Vm, call_id: u32, fn_id: u32| {
        assert_eq!(fn_id, PRINT, "unexpected host call");
        let args = vm.host_call(call_id).unwrap().args;
        let parts: Vec<String> = args.iter().map(|&v| vm.display(v)).collect();
        lines.push(parts.join(" "));
        vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
    };
    let v = super::dump_tests::run_plain_dump(vm, text, &host_fn, &mut print)?;
    Ok(vm.heap_mut().new_handle(v).unwrap())
}

/// Prints the suspension maps of a file, for reading what the exporter reads:
/// `DJS_SMAP=tests/dump/v2/x.src.js cargo test -p djs-core --lib show_maps -- --ignored --nocapture`.
#[test]
#[ignore]
fn show_maps() {
    let path = std::env::var("DJS_SMAP").expect("DJS_SMAP=file");
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path),
    )
    .unwrap();
    let mut vm = new_vm();
    let maps = crate::compiler::suspension_maps(
        vm.heap_mut(),
        src.as_bytes(),
        &crate::compiler::CompileOptions::script("src.js"),
    )
    .unwrap();
    for m in maps {
        std::print!("{}", m.text(src.as_bytes()));
    }
}

#[test]
fn dumps_need_an_idle_vm() {
    let opts = super::dump::DumpOptions {
        base: base(),
        host_fn_name: &host_fn_name,
        ..Default::default()
    };
    let reason = |vm: &Vm| match vm.dump(&opts, None) {
        Ok(t) => panic!("dumped:\n{t}"),
        Err(e) => e.message(),
    };
    // Parked on a sync host call.
    let mut vm = plain_vm();
    let f = vm
        .compile(
            b"print(1);",
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(vm.run(1_000_000), Status::HostCall { .. }));
    assert!(reason(&vm).contains("is not idle"), "{}", reason(&vm));
    // Out of fuel in the middle of a call.
    let mut vm = plain_vm();
    let f = vm
        .compile(
            b"for (let i = 0; i < 1e6; i++);",
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(vm.run(100), Status::OutOfFuel));
    assert!(reason(&vm).contains("is not idle"), "{}", reason(&vm));
    // A host result delivered and not applied yet.
    let mut vm = plain_vm();
    let f = vm
        .compile(
            b"globalThis.p = later(1);",
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(vm.run(1_000_000), Status::Idle | Status::Done(_)));
    let id = vm.pending_host_calls()[0];
    vm.resolve_host_call(id, Ok(Value::int(1))).unwrap();
    assert!(reason(&vm).contains("not applied yet"), "{}", reason(&vm));
    // A call of a host function the host does not name.
    let mut vm = plain_vm();
    let anon = vm.new_async_host_function(9, "anon").unwrap();
    vm.define_global("anon", anon).unwrap();
    let f = vm
        .compile(b"anon();", &crate::compiler::CompileOptions::script("t.js"))
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    vm.run(1_000_000);
    let r = reason(&vm);
    assert!(
        r.contains("<call 1>: a call of a host function the host does not name"),
        "{r}"
    );
    assert!(
        r.contains("anon: a host function the host does not name"),
        "{r}"
    );
}

/// example's scenario (0179): dumped during the first sleep, the report names both
/// activations and where they stopped; the text has no comments about them.
#[test]
fn the_report_names_where_activations_stopped() {
    let src = std::fs::read_to_string(fixture_dir().join("steps.src.js")).unwrap();
    let mut vm = plain_vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("steps.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(
        vm.run(1_000_000_000),
        Status::Idle | Status::Done(_)
    ));
    let opts = super::dump::DumpOptions {
        base: base(),
        host: &["later", "print"],
        host_fn_name: &host_fn_name,
        ..Default::default()
    };
    let d = vm.dump_report(&opts, None).unwrap();
    let got: Vec<(String, String, u32, u32, String)> = d
        .activations
        .iter()
        .map(|s| {
            (
                s.function.clone(),
                s.site.clone(),
                s.line,
                s.col,
                String::from(s.source_line.trim()),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "step".into(),
                "await#1".into(),
                6,
                3,
                "await sleep(ms);".into()
            ),
            (
                "main".into(),
                "await#1".into(),
                13,
                18,
                "results.push(await step(label, 10));".into()
            ),
        ]
    );
    assert_eq!(
        (d.open_calls, d.activations[0].file.as_str()),
        (1, "steps.js")
    );
    assert!(!d.text.contains("^\n"), "{}", d.text);
}

/// A codemode run (pi's Sandbox, `djs run --body`): the body, compiled as an async body
/// with the parameters `tools` and `console`, which the host also defines as globals.
const BODY: &str = "console.log(\"start\");\nlet n = 0;\nfor (const f of await tools.list()) {\n  \
                    const body = await tools.read(f);\n  n += body.length;\n  console.log(f, body);\n}\n\
                    const total = () => n;\nawait later(0);\nconsole.log(\"total\", total());\nreturn n;\n";

/// A VM with the codemode host's `tools` and `console` (made by host code).
fn body_host_vm() -> Vm {
    body_host_vm_with(false)
}

/// ... collecting at every safepoint with `stress`.
fn body_host_vm_with(stress: bool) -> Vm {
    let mut vm = vm_with(VmOptions {
        frozen_builtins: true,
        exec: ExecOptions {
            gc_stress: stress,
            ..ExecOptions::default()
        },
        ..VmOptions::default()
    });
    run_value(
        &mut vm,
        "globalThis.tools = { list: () => later([\"a\", \"b\"]), read: (f) => later(f + f) };\n\
         globalThis.console = { log: print }; 0",
    );
    vm
}

/// The body run up to idle point `k` (all of it with `usize::MAX`): its lines, the
/// run's promise and the dump there.
fn body_at(k: usize) -> (Vec<String>, Option<super::dump::Dumped>) {
    let (lines, d) = body_dump(BODY, k);
    (
        lines,
        d.map(|d| d.unwrap_or_else(|e| panic!("idle point {k}: {e}"))),
    )
}

/// [`body_at`] for body `src`: the dump, or why it was refused.
#[allow(clippy::type_complexity)]
fn body_dump(src: &str, k: usize) -> (Vec<String>, Option<Result<super::dump::Dumped, String>>) {
    let mut vm = body_host_vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::async_body("body.js", &["tools", "console"]),
        )
        .unwrap();
    let g = vm.global();
    let args = ["tools", "console"].map(|n| {
        let k = vm.heap_mut().atom_str(n).unwrap();
        vm.heap().get_own_property(g, k).unwrap().value().unwrap()
    });
    vm.call(f, Value::object(g), &args).unwrap();
    let (mut lines, mut seen, mut run) = (Vec::new(), 0, None);
    loop {
        match vm.run(1_000_000_000) {
            Status::HostCall { call_id, .. } => {
                let args = vm.host_call(call_id).unwrap().args;
                let parts: Vec<String> = args.iter().map(|&v| vm.display(v)).collect();
                lines.push(parts.join(" "));
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::Idle | Status::Done(_) => {
                // The run's promise (`Idle` after the call returned, while it waits).
                if run.is_none() {
                    // Held by a handle: nothing else does once the run is over.
                    let v = state::result(&vm.heap);
                    run = Some(vm.heap_mut().new_handle(v).unwrap());
                }
                let Some(&id) = vm.pending_host_calls().iter().min() else {
                    let r = vm.promise_state(vm.heap().get_handle(run.unwrap()));
                    let r = r.flatten().map(|r| r.map(|v| vm.display(v)));
                    lines.push(alloc::format!("run {r:?}"));
                    return (lines, None);
                };
                seen += 1;
                if seen == k {
                    break;
                }
                settle(&mut vm, id);
            }
            Status::Threw(e) => {
                lines.push(alloc::format!("threw {}", vm.display(e)));
                return (lines, None);
            }
            s => panic!("{s:?}"),
        }
    }
    let handles = [("run", vm.heap().get_handle(run.unwrap()))];
    let opts = super::dump::DumpOptions {
        base: base(),
        host: &["console", "tools", "print", "later"],
        host_roots: &[],
        host_fn_name: &host_fn_name,
        handles: &handles,
        call_states: &[],
        source_refs: false,
        base_names: None,
        check: true,
    };
    let d = vm.dump_report(&opts, None).map_err(|e| e.message());
    (lines, Some(d))
}

/// The run's own activation is written as a continuation `async function (tools,
/// console) {...}` at every idle point; the report points into the body.
#[test]
fn a_codemode_body_run_round_trips() {
    let (want, _) = body_at(usize::MAX);
    assert_eq!(want.last().unwrap(), "run Some(Ok(\"4\"))", "{want:?}");
    for k in 1..=4 {
        let (mut lines, d) = body_at(k);
        let d = d.unwrap();
        if std::env::var("DJS_DUMP_SHOW").is_ok_and(|v| v == alloc::format!("body@{k}")) {
            std::println!("{}", d.text);
        }
        assert!(
            d.text.contains("async function (tools, console) {"),
            "{}",
            d.text
        );
        let mut vm = body_host_vm();
        let run = plain_run(&mut vm, &d.text, &mut lines)
            .unwrap_or_else(|e| panic!("@{k}: {e}\n{}", d.text));
        let (mut after, mut marks) = (Vec::new(), Vec::new());
        drive(&mut vm, &mut after, &mut marks, None);
        lines.extend(after);
        let run = vm.heap().get_handle(run);
        let r = vm
            .promise_state(run)
            .flatten()
            .map(|r| r.map(|v| vm.display(v)));
        lines.push(alloc::format!("run {r:?}"));
        assert_eq!(lines, want, "@{k}\n{}", d.text);
        let at: Vec<(String, u32, u32, String)> = d
            .activations
            .iter()
            .map(|s| {
                (
                    s.site.clone(),
                    s.line,
                    s.col,
                    String::from(s.source_line.trim()),
                )
            })
            .collect();
        let want_at = match k {
            1 => ("await#1", 3, 17, "for (const f of await tools.list()) {"),
            2 | 3 => ("await#2", 4, 16, "const body = await tools.read(f);"),
            _ => ("await#3", 9, 1, "await later(0);"),
        };
        assert_eq!(
            at,
            [(want_at.0.into(), want_at.1, want_at.2, want_at.3.into())],
            "@{k}"
        );
        assert_eq!(d.activations[0].file, "body.js");
    }
}

/// A body whose code would mean something else as a function is refused: `arguments`
/// is a global in a body and the call's arguments in a function.
#[test]
fn a_codemode_body_that_differs_as_a_function_is_refused() {
    let (_, d) = body_dump("await later(1); console.log(typeof arguments);", 1);
    let e = d.unwrap().unwrap_err();
    assert!(
        e.contains("a codemode body whose frame at await#1 differs from the async function"),
        "{e}"
    );
    // `this` is the global object in both.
    let src = "const t = this; await later(1); console.log(t === globalThis); return 5";
    let (want, _) = body_dump(src, usize::MAX);
    let (mut lines, d) = body_dump(src, 1);
    let d = d.unwrap().unwrap();
    let mut vm = body_host_vm();
    plain_run(&mut vm, &d.text, &mut lines).unwrap();
    drive(&mut vm, &mut lines, &mut Vec::new(), None);
    assert_eq!(lines, want[..want.len() - 1]);
    assert_eq!(want, ["true", "run Some(Ok(\"5\"))"]);
}

// ------------------------------------------------------------------ golden dumps (0178)
//
// tests/dump/v2/golden/: small programs (`NAME.src.js` Scripts, `NAME.body.js` codemode
// bodies) and the dumps the exporter wrote of them, `NAME.K.dump.js` at idle point K.
// Every later build must run them and finish the run as the program does (D2560, D2920):
// they are JavaScript that calls the dump runtime, so this holds the runtime and the
// engine to the format, not the exporter. A golden dump changes only with a decision
// (D2642). The host is `body_host_vm`'s: `later`, `print`, `tools`, `console`. Write the dumps of a new program (never over existing
// ones) with `DJS_GOLDEN_WRITE=1 cargo test -p djs-core --lib write_golden_dumps --
// --ignored`.

const GOLDEN_HOST: &[&str] = &["console", "tools", "print", "later"];

fn golden_dir() -> std::path::PathBuf {
    fixture_dir().join("golden")
}

/// The golden programs: name, whether it is a codemode body, source.
fn golden_programs() -> Vec<(String, bool, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(golden_dir()).unwrap() {
        let file = e.unwrap().file_name().into_string().unwrap();
        let (name, body) = match (file.strip_suffix(".src.js"), file.strip_suffix(".body.js")) {
            (Some(n), _) => (n, false),
            (_, Some(n)) => (n, true),
            _ => continue,
        };
        let src = std::fs::read_to_string(golden_dir().join(&file)).unwrap();
        out.push((String::from(name), body, src));
    }
    out.sort();
    out
}

/// The golden dumps: program name, idle point, file name.
fn golden_dumps() -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(golden_dir()).unwrap() {
        let file = e.unwrap().file_name().into_string().unwrap();
        let Some(stem) = file.strip_suffix(".dump.js") else {
            continue;
        };
        let (name, k) = stem.rsplit_once('.').unwrap();
        out.push((String::from(name), k.parse().unwrap(), file));
    }
    out.sort();
    out
}

/// Starts golden program `src` in a host VM: a Script, or a body called with the host's
/// `tools` and `console`.
fn golden_start(src: &str, body: bool) -> Vm {
    let mut vm = body_host_vm();
    let g = vm.global();
    if body {
        let f = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::async_body("body.js", &["tools", "console"]),
            )
            .unwrap();
        let args = ["tools", "console"].map(|n| {
            let k = vm.heap_mut().atom_str(n).unwrap();
            vm.heap().get_own_property(g, k).unwrap().value().unwrap()
        });
        vm.call(f, Value::object(g), &args).unwrap();
    } else {
        let f = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::script("src.js"),
            )
            .unwrap_or_else(|e| panic!("{} at {}:{}", e.message, e.line, e.col));
        vm.call(f, Value::object(g), &[]).unwrap();
    }
    vm
}

/// Runs to idle point `k` (true) or to the end, as `drive` does. `run` is the run's
/// promise; with `entry`, the entry call's result at the first stop (a body's promise).
/// At the end, the promise's state is a line of its own.
fn golden_drive(
    vm: &mut Vm,
    k: usize,
    entry: bool,
    run: &mut Option<crate::heap::Handle>,
    lines: &mut Vec<String>,
    marks: &mut Vec<usize>,
    slice: Option<u64>,
) -> bool {
    loop {
        match vm.run(slice.unwrap_or(1_000_000_000)) {
            Status::OutOfFuel if slice.is_some() => {}
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, PRINT, "unexpected host call");
                let args = vm.host_call(call_id).unwrap().args;
                let parts: Vec<String> = args.iter().map(|&v| vm.display(v)).collect();
                lines.push(parts.join(" "));
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::Idle | Status::Done(_) => {
                if entry && run.is_none() {
                    let v = state::result(&vm.heap);
                    *run = Some(vm.heap_mut().new_handle(v).unwrap());
                }
                let Some(&id) = vm.pending_host_calls().iter().min() else {
                    break;
                };
                marks.push(lines.len());
                if marks.len() == k {
                    return true;
                }
                settle(vm, id);
            }
            Status::Threw(e) => {
                let m = vm.display(e);
                lines.push(alloc::format!("threw {m}"));
                break;
            }
            s => panic!("{s:?}"),
        }
    }
    for r in vm.take_unhandled_rejections() {
        let m = vm.display(r);
        lines.push(alloc::format!("unhandled {m}"));
    }
    let v = run.map_or(Value::UNDEFINED, |h| vm.heap().get_handle(h));
    if let Some(r) = vm.promise_state(v) {
        let r = r.map(|r| r.map(|v| vm.display(v)));
        lines.push(alloc::format!("run {r:?}"));
    }
    false
}

/// A golden program's uninterrupted run: its lines and idle marks.
fn golden_run(src: &str, body: bool) -> (Vec<String>, Vec<usize>) {
    let mut vm = golden_start(src, body);
    let (mut lines, mut marks) = (Vec::new(), Vec::new());
    golden_drive(
        &mut vm,
        usize::MAX,
        body,
        &mut None,
        &mut lines,
        &mut marks,
        None,
    );
    (lines, marks)
}

/// Runs golden dump `text` and finishes the run: the lines it prints.
fn golden_continue(text: &str, stress: bool) -> Result<Vec<String>, String> {
    let mut vm = body_host_vm_with(stress);
    let (mut lines, mut marks) = (Vec::new(), Vec::new());
    let mut run = Some(plain_run(&mut vm, text, &mut lines)?);
    golden_drive(
        &mut vm,
        usize::MAX,
        false,
        &mut run,
        &mut lines,
        &mut marks,
        stress.then_some(97),
    );
    Ok(lines)
}

/// Every golden dump loads and finishes as its program does, also under GC stress and
/// in slices; every program has a dump.
#[test]
fn golden_dumps_load_and_finish() {
    let programs = golden_programs();
    let dumps = golden_dumps();
    let mut failed = Vec::new();
    // A program whose idle points are all refused (D2878) has none: it is listed in
    // plain-refusals.txt.
    let refusals = std::fs::read_to_string(plain_refusals_file()).unwrap_or_default();
    for (name, _, _) in &programs {
        let listed = refusals.contains(&alloc::format!("golden/{name}@"));
        if !dumps.iter().any(|d| d.0 == *name) && !listed {
            failed.push(alloc::format!("{name}: no dump"));
        }
    }
    for (name, k, file) in &dumps {
        let Some((_, body, src)) = programs.iter().find(|p| p.0 == *name) else {
            failed.push(alloc::format!("{file}: no program {name}"));
            continue;
        };
        let text = std::fs::read_to_string(golden_dir().join(file)).unwrap();
        let (want, marks) = golden_run(src, *body);
        if *k == 1 && std::env::var("DJS_DUMP_SHOW").is_ok_and(|v| v == "golden") {
            std::println!("{name}: {} idle points, {want:?}", marks.len());
        }
        if !text.starts_with("\"djs-dump\";\n") {
            failed.push(alloc::format!("{file}: not a dump"));
            continue;
        }
        if *k == 0 || *k > marks.len() {
            failed.push(alloc::format!(
                "{file}: {name} has {} idle points",
                marks.len()
            ));
            continue;
        }
        for stress in [false, true] {
            match golden_continue(&text, stress) {
                Ok(after) => {
                    let mut got = want[..marks[k - 1]].to_vec();
                    got.extend(after);
                    if got != want {
                        failed.push(alloc::format!(
                            "{file} (stress {stress}): {got:?}\n  want {want:?}"
                        ));
                    }
                }
                Err(e) => failed.push(alloc::format!("{file}: {e}")),
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Writes the dumps of the golden programs that have none, at every idle point.
#[test]
#[ignore]
fn write_golden_dumps() {
    assert!(
        std::env::var_os("DJS_GOLDEN_WRITE").is_some(),
        "DJS_GOLDEN_WRITE=1 writes golden dumps"
    );
    let dumps = golden_dumps();
    for (name, body, src) in golden_programs() {
        if dumps.iter().any(|d| d.0 == name) {
            continue;
        }
        let (_, marks) = golden_run(&src, body);
        for k in 1..=marks.len() {
            let mut vm = golden_start(&src, body);
            let mut run = None;
            assert!(golden_drive(
                &mut vm,
                k,
                body,
                &mut run,
                &mut Vec::new(),
                &mut Vec::new(),
                None
            ));
            let run = run.map_or(Value::UNDEFINED, |h| vm.heap().get_handle(h));
            let handles = [("run", run)];
            let opts = super::dump::DumpOptions {
                base: base(),
                host: GOLDEN_HOST,
                host_roots: &[],
                host_fn_name: &host_fn_name,
                handles: if body { &handles } else { &[] },
                call_states: &[],
                source_refs: false,
                base_names: None,
                check: true,
            };
            let text = match vm.dump(&opts, None) {
                Ok(t) => t,
                Err(e) => {
                    std::println!("refused {name} @{k}: {}", e.message());
                    continue;
                }
            };
            let file = golden_dir().join(alloc::format!("{name}.{k}.dump.js"));
            std::fs::write(&file, text).unwrap();
            std::println!("wrote {}", file.display());
        }
    }
}

/// The common pi shape: a closure over a body's parameter makes it a boxed local, which
/// must sit where the async function the dump writes keeps it (D2620; 0174's repro).
#[test]
fn a_codemode_body_whose_parameter_a_closure_captures() {
    let p = ["tools", "console"];
    let body = "const a = await later(1);\nfunction f() { tools; }\n";
    let wrapped = alloc::format!("(async function (tools, console) {{\n{body}\n}})");
    let mut vm = new_vm();
    let site = |vm: &mut Vm, src: &str, o: &crate::compiler::CompileOptions<'_>| {
        let maps = crate::compiler::suspension_maps(vm.heap_mut(), src.as_bytes(), o).unwrap();
        let s = maps.last().unwrap().sites[0].clone();
        (s.args, s.locals, s.boxed)
    };
    let a = site(
        &mut vm,
        body,
        &crate::compiler::CompileOptions::async_body("b.js", &p),
    );
    let b = site(
        &mut vm,
        &wrapped,
        &crate::compiler::CompileOptions::script("d.js"),
    );
    assert_eq!(a, b);
    let src = "const t = tools;\nconst a = await later(1);\nconst f = (k) => tools === t && k;\n\
               console.log(f(a), await tools.read(\"x\"));\nreturn a;\n";
    let (want, _) = body_dump(src, usize::MAX);
    assert_eq!(want, ["1 xx", "run Some(Ok(\"1\"))"]);
    for k in 1..=2 {
        let (mut lines, d) = body_dump(src, k);
        let d = d.unwrap().unwrap_or_else(|e| panic!("@{k}: {e}"));
        let mut vm = body_host_vm();
        let run = plain_run(&mut vm, &d.text, &mut lines).unwrap();
        drive(&mut vm, &mut lines, &mut Vec::new(), None);
        let run = vm.heap().get_handle(run);
        let r = vm
            .promise_state(run)
            .flatten()
            .map(|r| r.map(|v| vm.display(v)));
        lines.push(alloc::format!("run {r:?}"));
        assert_eq!(lines, want, "@{k}");
    }
}

/// `const r = await ...` while the await is pending: a binding in its TDZ (code reads it
/// before the declaration), captured or not, is a declaration of the continuation that
/// has not run yet; one no read can come first holds undefined (0179's question).
#[test]
fn bindings_in_their_tdz_at_an_await() {
    for src in [
        "async function main() { try { const r = (await later(1)) + r; print(r); } catch (e) { print(e.name); } }\nmain();",
        "async function main() { for (let i = 0; i < 2; i++) { if (i) { try { print(r); } catch (e) { print(e.name); } } const r = await later(i); } }\nmain();",
        "async function main() { const r = await later(1); print(r); }\nmain();",
        // A closure over the binding, made in K and held by a global `let`: declared at
        // the top, assigned in the callback (0209).
        "let get; later(2).then(() => print(get()));\nasync function main() { get = () => { try { return r; } catch (e) { return e.name; } }; const r = await later(1); print(r); }\nmain();",
    ] {
        let (got, text) = dump_at(src, 1).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(got, run_program(src).0, "{src}\n{text}");
        assert!(got[0] == "ReferenceError" || got[0] == "1", "{got:?}");
    }
}

/// A class with instance fields declared in a waiting function: the frame's hidden
/// bindings of the class are dead after its statement; the class comes from its text
/// (0175's repro). Instances with private names are refused (D2878).
#[test]
fn classes_with_fields_in_a_waiting_frame() {
    for src in [
        "class C { x = 1; } const c = new C(); await later(1); return c.x;",
        "class C { static s = 1; kk = 2; get m() { return 3; } }\n\
         const c = new C(); await later(1); return c.kk + c.m + C.s;",
        "const out = []; for (const v of [1, 2]) { class C { y = v; } out.push(new C().y + (await later(v))); } return out.join();",
    ] {
        let (want, _) = body_dump(src, usize::MAX);
        assert!(!want.last().unwrap().contains("Err"), "{src}: {want:?}");
        for k in 1..=marks_of(src) {
            let (mut lines, d) = body_dump(src, k);
            let d = d.unwrap().unwrap_or_else(|e| panic!("{src} @{k}: {e}"));
            let mut vm = body_host_vm();
            let run = plain_run(&mut vm, &d.text, &mut lines)
                .unwrap_or_else(|e| panic!("{src} @{k}: {e}"));
            drive(&mut vm, &mut lines, &mut Vec::new(), None);
            let run = vm.heap().get_handle(run);
            let r = vm
                .promise_state(run)
                .flatten()
                .map(|r| r.map(|v| vm.display(v)));
            lines.push(alloc::format!("run {r:?}"));
            assert_eq!(lines, want, "{src} @{k}\n{}", d.text);
        }
    }
    let src = "class C { #n = 0; inc() { return ++this.#n; } } const c = new C(); await later(1); return c.inc();";
    let e = body_dump(src, 1).1.unwrap().unwrap_err();
    assert!(e.contains("with private name #n"), "{e}");
}

/// The number of idle points of body `src`.
fn marks_of(src: &str) -> usize {
    (1..).find(|&k| body_dump(src, k).1.is_none()).unwrap() - 1
}

/// A class's or object literal method's computed key is evaluated again where the dump
/// writes its text: refused unless its value cannot differ (D2623; 0182).
#[test]
fn computed_keys_evaluated_again_are_refused() {
    for (src, why) in [
        (
            "const k = \"kk\"; class C { [k] = 2; } const c = new C(); later(1).then(() => print(c.kk, new C().kk));",
            "C: a computed key [k] that the dump would evaluate again",
        ),
        (
            "function mk() { const k = \"kk\"; return class C { [k]() { return 2; } }; } const C = mk(); later(1).then(() => print(new C().kk()));",
            "C: a computed key [k] that the dump would evaluate again",
        ),
        (
            "function mk() { const k = \"kk\"; return { [k]() { return 2; } }; } const o = mk(); later(1).then(() => print(o.kk()));",
            "o.kk: a computed key [k] that the dump would evaluate again",
        ),
    ] {
        let e = dump_at(src, 1).unwrap_err();
        assert!(e.contains(why), "{src}: {e}");
    }
    // Literal keys and well-known symbols; a computed key inside a method body is the
    // method's own.
    let src = "class C { [Symbol.iterator]() { return [1][Symbol.iterator](); } [\"a b\"]() { return 3; } [0] = 1;\n\
               m() { const o = { [x]: 1 }; return o; } }\n\
               const x = 'q'; const c = new C(); later(1).then(() => print([...c].join(), c['a b'](), JSON.stringify(c.m()), c[0]));";
    assert_eq!(dump_at(src, 1).unwrap().0, ["1 3 {\"q\":1} 1"]);
}

/// The run of `src` dumped at every idle point and continued in a fresh VM that ran the
/// dump, dump after dump (as `--plain-dumps`): its lines.
fn chained(src: &str) -> Vec<String> {
    let mut vm = plain_vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("src.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut lines = Vec::new();
    for _ in 0..1000 {
        if plain_step(&mut vm, &mut lines).is_none() {
            return lines;
        }
        let text = plain_dump(&vm).unwrap_or_else(|e| panic!("{e}"));
        vm = plain_load(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        let id = plain_step(&mut vm, &mut lines).expect("an idle point");
        settle(&mut vm, id);
    }
    panic!("runaway: {lines:?}");
}

/// `Math.random`'s state goes with the dump: a resumed run continues the sequence
/// (0176's repro, D2624); iterators stepped by `$djs.advance` dump again as they were
/// (two awaits per iteration: dumped again before the loop steps them).
#[test]
fn dumps_carry_the_random_state_and_iterators_again() {
    for src in [
        "async function main() { const a = Math.random(); await later(1); const b = Math.random(); await later(2); print(a, b, Math.random()); }\nmain();",
        "async function main() { const s = new Set([1, 2, 3]).values(); s.next(); for (const [k, v] of [[\"a\", 1], [\"b\", 2], [\"c\", 3]]) print(k, await later(v), await later(v + 1), s.next().value); for (const c of \"h\\u00e9\\u{1F600}x\") print(await later(c)); }\nmain();",
        // (Math.random first after the dump: no state written, the loading VM's own
        // seed continues as the uninterrupted run's, D2837.)
        "async function main() { await later(1); print(Math.random()); await later(2); print(Math.random()); }\nmain();",
    ] {
        let (want, _) = run_program(src);
        assert_eq!(chained(src), want, "{src}");
    }
    // The state only where Math.random ran (D2837); a re-dump of a loaded state writes it
    // again (the chains above).
    let seeded = |src: &str| dump_at(src, 1).unwrap().1.contains("$djs.seed(");
    assert!(seeded(
        "async function main() { Math.random(); await later(1); }\nmain();"
    ));
    assert!(!seeded(
        "async function main() { await later(1); Math.random(); }\nmain();"
    ));
}

/// A for-in loop over a Proxy walks a Record its traps fill: refused.
#[test]
fn a_for_in_over_a_proxy_is_refused() {
    let src = "async function main() { for (const k in new Proxy({ a: 1 }, {})) print(k, await later(k)); }\nmain();";
    let e = dump_at(src, 1).unwrap_err();
    assert!(e.contains("a for-in loop over a Proxy"), "{e}");
}

/// An array with named properties is not a literal alone: it gets a location for its
/// patches even where a frame holds it once.
#[test]
fn a_frame_holds_an_array_with_named_properties() {
    let src =
        "async function main() { const a = [1]; a.x = 2; print(await later(1), a.x); }\nmain();";
    let (got, _) = dump_at(src, 1).unwrap();
    assert_eq!(got, ["1 2"]);
}

/// Runs the VM to its next idle point (printing): `Some(oldest open call)`, or `None` at
/// the end (with "threw ..." for an uncaught error).
fn plain_step(vm: &mut Vm, lines: &mut Vec<String>) -> Option<u32> {
    loop {
        match vm.run(1_000_000_000) {
            Status::HostCall { call_id, fn_id } if fn_id == SEED => {
                let args = vm.host_call(call_id).unwrap().args;
                // (`$djs.seed(lo, hi[, next])` or `$djs.seed(next)`, D2837.)
                let half = |v: Value| v.as_number().unwrap_or(0.0) as i32 as u32 as u64;
                if args.get(1).is_some_and(|v| !v.is_undefined()) {
                    vm.restore_math_random((half(args[1]) << 32) | half(args[0]));
                }
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, PRINT, "unexpected host call");
                let args = vm.host_call(call_id).unwrap().args;
                let parts: Vec<String> = args.iter().map(|&v| vm.display(v)).collect();
                lines.push(parts.join(" "));
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::Idle | Status::Done(_) => return vm.pending_host_calls().iter().min().copied(),
            Status::Threw(e) => {
                let m = vm.display(e);
                lines.push(alloc::format!("threw {m}"));
                return None;
            }
            s => panic!("{s:?}"),
        }
    }
}

/// A plain dump (D2830) of the idle `vm`, or why it was refused.
fn plain_dump(vm: &Vm) -> Result<String, String> {
    // (The base's names as hosts that dump have them, D3020: the base is not read.)
    let opts = super::dump::DumpOptions {
        base: &[],
        host: &["console", "tools", "print", "later"],
        host_fn_name: &host_fn_name,
        base_names: Some(super::dump_harvest::base_names()),
        ..Default::default()
    };
    vm.dump(&opts, None).map_err(|e| e.message())
}

/// The plain dump of program `src` run to idle point `k`.
fn plain_dump_at(src: &str, k: usize) -> String {
    let mut vm = plain_vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("src.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut lines = Vec::new();
    for _ in 1..k {
        let id = plain_step(&mut vm, &mut lines).expect("an idle point");
        settle(&mut vm, id);
    }
    plain_step(&mut vm, &mut lines).expect("an idle point");
    plain_dump(&vm).unwrap()
}

/// A fresh VM that ran plain dump `text` as a Script, with the dump runtime and the host's
/// hooks on `$djs` (D2903): `$djs.seed` a no-op (the fixtures draw no random numbers),
/// `$djs.resume` making the call again, `$djs.host` the global of the name's path.
fn plain_load(text: &str) -> Result<Vm, String> {
    let mut vm = plain_vm();
    let seed = vm.new_host_function(SEED, "seed").unwrap();
    vm.define_global("$djs_seed", seed).unwrap();
    let rt = vm.dump_runtime().unwrap();
    vm.define_global("$djs", rt).unwrap();
    run_value(
        &mut vm,
        "Object.assign($djs, { seed: $djs_seed, resume: (id, name, args) => later(...args), host: (n) => n.split('.').reduce((o, k) => o[k], globalThis) }); delete globalThis.$djs_seed; 0",
    );
    let f = vm
        .compile(
            text.as_bytes(),
            &crate::compiler::CompileOptions::script("dump.js"),
        )
        .map_err(|e| alloc::format!("{} at {}:{}", e.message, e.line, e.col))?;
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).map_err(|e| alloc::format!("{e:?}"))?;
    loop {
        match vm.run(1_000_000_000) {
            Status::Idle | Status::Done(_) => break,
            Status::HostCall { call_id, fn_id } if fn_id == SEED => {
                let args = vm.host_call(call_id).unwrap().args;
                // (`$djs.seed(lo, hi[, next])` or `$djs.seed(next)`, D2837.)
                let half = |v: Value| v.as_number().unwrap_or(0.0) as i32 as u32 as u64;
                if args.get(1).is_some_and(|v| !v.is_undefined()) {
                    vm.restore_math_random((half(args[1]) << 32) | half(args[0]));
                }
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::Threw(e) => return Err(alloc::format!("the load threw {}", vm.display(e))),
            s => return Err(alloc::format!("the load stopped: {s:?}")),
        }
    }
    // (The host deletes them, D2916.)
    vm.end_dump_load().map_err(|e| alloc::format!("{e:?}"))?;
    if run_value(&mut vm, "typeof $dump + typeof $djs") != "undefinedundefined" {
        return Err("the load left $dump or $djs".into());
    }
    Ok(vm)
}

/// Every fixture program, plain-dumped at every idle point of a chain: the run dumped at
/// its first idle point, the loaded dump at its first, and so on. The lines must be the
/// uninterrupted run's; a refusal ends the chain (counted: the constructs 0203 adds).
/// The programs plain dumps are checked on: the v2 fixtures and the golden programs.
fn plain_programs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for name in [
        "plan",
        "operands",
        "generator",
        "asyncgen",
        "combinators",
        "loops",
        "cells",
        "steps",
        "reenact",
        "cycles",
        "supers",
    ] {
        let src =
            std::fs::read_to_string(fixture_dir().join(alloc::format!("{name}.src.js"))).unwrap();
        out.push((String::from(name), src));
    }
    for (name, body, src) in golden_programs() {
        if !body {
            out.push((alloc::format!("golden/{name}"), src));
        }
    }
    out
}

/// The refusals plain dumps of the programs may give, `program@k: reason` (a ratchet:
/// `DJS_UPDATE_PLAIN_REFUSALS=1` rewrites it).
fn plain_refusals_file() -> std::path::PathBuf {
    fixture_dir().join("plain-refusals.txt")
}

/// Every program dumped (plainly) at each of its idle points, the dump run in a fresh VM
/// to the end: the lines before and after must be the uninterrupted run's. Refusals are
/// checked against tests/dump/v2/plain-refusals.txt.
/// Program `src` dumped plainly at each of its idle points, each dump run in a fresh VM to
/// the end: the failures (`name@k: ...`), the refusals and how many continued identically.
fn plain_check_program(name: &str, src: &str) -> (Vec<String>, Vec<String>, usize) {
    let (mut failed, mut refused, mut ok) = (Vec::new(), Vec::new(), 0);
    let (want, marks) = run_program(src);
    for k in 1..=marks.len() {
        let mut vm = plain_vm();
        let f = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::script("src.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
        let mut lines = Vec::new();
        for _ in 1..k {
            let id = plain_step(&mut vm, &mut lines).expect("an idle point");
            settle(&mut vm, id);
        }
        plain_step(&mut vm, &mut lines).expect("an idle point");
        let text = match plain_dump(&vm) {
            Ok(t) => t,
            Err(e) => {
                let why: Vec<&str> = e.lines().skip(1).map(str::trim).collect();
                refused.push(alloc::format!("{name}@{k}: {}", why.join(" | ")));
                continue;
            }
        };
        if std::env::var("DJS_PLAIN_SHOW").is_ok_and(|v| v == alloc::format!("{name}@{k}")) {
            std::println!("{text}");
        }
        // `DJS_PLAIN_OUT=DIR`: every dump goes to DIR/<program>/<k>.js (0207's size
        // measurement, scripts/dump-size).
        if let Ok(dir) = std::env::var("DJS_PLAIN_OUT") {
            let dir = std::path::Path::new(&dir).join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(alloc::format!("{k}.js")), &text).unwrap();
        }
        let mut vm = match plain_load(&text) {
            Ok(vm) => vm,
            Err(e) => {
                failed.push(alloc::format!("{name}@{k}: {e}\n{text}"));
                continue;
            }
        };
        while let Some(id) = plain_step(&mut vm, &mut lines) {
            settle(&mut vm, id);
        }
        if lines != want {
            failed.push(alloc::format!(
                "{name}@{k}: {lines:?}\n  want {want:?}\n{text}"
            ));
        } else {
            ok += 1;
        }
    }
    (failed, refused, ok)
}

#[test]
fn plain_dumps_at_every_idle_point() {
    let (mut failed, mut refused, mut ok) = (Vec::new(), Vec::new(), 0);
    for (name, src) in plain_programs() {
        let (f, r, n) = plain_check_program(&name, &src);
        failed.extend(f);
        refused.extend(r);
        ok += n;
    }
    let path = plain_refusals_file();
    if std::env::var("DJS_UPDATE_PLAIN_REFUSALS").is_ok() {
        let mut t = String::from(
            "# Idle points of tests/dump/v2 whose plain dump is refused (plain_dumps_at_every_idle_point).\n",
        );
        for r in &refused {
            t.push_str(r);
            t.push('\n');
        }
        std::fs::write(&path, t).unwrap();
    }
    let listed = std::fs::read_to_string(&path).unwrap_or_default();
    let listed: Vec<&str> = listed.lines().filter(|l| !l.starts_with('#')).collect();
    for r in &refused {
        if !listed.contains(&r.as_str()) {
            failed.push(alloc::format!("refused, not in plain-refusals.txt: {r}"));
        }
    }
    for l in &listed {
        if !refused.iter().any(|r| r == l) {
            std::println!("stale in plain-refusals.txt: {l}");
        }
    }
    std::println!("plain dumps: {ok} identical, {} refused", refused.len());
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Program `src` dumped plainly at its first idle point, the dump run to its first, dumped
/// again, and so on: the failures, the links and the refusal that ended the chain.
fn plain_chain_program(name: &str, src: &str) -> (Vec<String>, usize, Vec<String>) {
    let (mut failed, mut links, mut refused) = (Vec::new(), 0, Vec::new());
    let (want, _) = run_program(src);
    let mut vm = plain_vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("src.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut lines = Vec::new();
    let mut dumping = true;
    let show = std::env::var("DJS_PLAIN_SHOW").is_ok_and(|v| v == name);
    let mut sizes = Vec::new();
    while let Some(id) = plain_step(&mut vm, &mut lines) {
        if dumping {
            match plain_dump(&vm) {
                Ok(text) => match plain_load(&text) {
                    _ if show && {
                        std::println!("--- {name} link {}\n{text}", links + 1);
                        false
                    } =>
                    {
                        unreachable!()
                    }
                    Ok(v) => {
                        links += 1;
                        sizes.push(text.len());
                        vm = v;
                        let id = vm.pending_host_calls().iter().min().copied();
                        settle(&mut vm, id.expect("the dump's open call"));
                        continue;
                    }
                    Err(e) => {
                        failed.push(alloc::format!("{name} link {}: {e}\n{text}", links + 1));
                        break;
                    }
                },
                Err(e) => {
                    let why: Vec<&str> = e.lines().skip(1).map(str::trim).collect();
                    refused.push(alloc::format!("{name}: {}", why.join(" | ")));
                    dumping = false;
                }
            }
        }
        settle(&mut vm, id);
    }
    if lines != want {
        failed.push(alloc::format!("{name}: {lines:?}\n  want {want:?}"));
    }
    // Origin tracking keeps a chain's dumps the size of their state (D2870): a
    // continuation of a continuation is written from the original's text, so link k
    // is about as long as a dump of the uninterrupted run at idle point k.
    // Objects the dump runtime made for the previous load are written as its helper
    // calls again (D2951): no link carries the runtime's code.
    for (k, &len) in sizes.iter().enumerate() {
        let fresh = plain_dump_at(src, k + 1).len();
        // (A link may be smaller: a binding its continuation leaves out as dead, D2911,
        // keeps no data in the next dump.)
        if len > fresh + 512 {
            failed.push(alloc::format!(
                "{name}: link {} is {len} bytes, a dump of the run there {fresh}",
                k + 1
            ));
        }
    }
    (failed, links, refused)
}

#[test]
fn plain_dump_chains_continue_like_the_run() {
    let (mut failed, mut links, mut refused) = (Vec::new(), 0, Vec::new());
    for (name, src) in plain_programs() {
        let (f, n, r) = plain_chain_program(&name, &src);
        failed.extend(f);
        links += n;
        refused.extend(r);
    }
    // Where chains stop: a ratchet like the single dumps' (the same variable updates it).
    let path = fixture_dir().join("plain-chain-refusals.txt");
    if std::env::var("DJS_UPDATE_PLAIN_REFUSALS").is_ok() {
        let mut t = String::from(
            "# Programs of tests/dump/v2 whose plain-dump chain is refused at some link (plain_dump_chains_continue_like_the_run).\n",
        );
        for r in &refused {
            t.push_str(r);
            t.push('\n');
        }
        std::fs::write(&path, t).unwrap();
    }
    let listed = std::fs::read_to_string(&path).unwrap_or_default();
    let listed: Vec<&str> = listed.lines().filter(|l| !l.starts_with('#')).collect();
    for r in &refused {
        if !listed.contains(&r.as_str()) {
            failed.push(alloc::format!(
                "refused, not in plain-chain-refusals.txt: {r}"
            ));
        }
    }
    for l in &listed {
        if !refused.iter().any(|r| r == l) {
            std::println!("stale in plain-chain-refusals.txt: {l}");
        }
    }
    std::println!(
        "plain dumps: {links} links, {} chains refused",
        refused.len()
    );
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(links >= 10, "{links} links");
}

/// A plain continuation's `super` is read again (K is a method of an object whose
/// prototype is the home object's): a base read before the dump that the home object no
/// longer has is refused, not changed.
#[test]
fn a_super_base_changed_since_it_was_read_is_refused() {
    let src = "class A { m(x) { return 'a' + x; } }\nclass B { m(x) { return 'b' + x; } }\n\
               class C extends A { async m() { return super[await later('m')](1); } }\n\
               new C().m().then(print);\nObject.setPrototypeOf(C.prototype, B.prototype);";
    let mut vm = plain_vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("src.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut lines = Vec::new();
    plain_step(&mut vm, &mut lines).expect("an idle point");
    let e = plain_dump(&vm).unwrap_err();
    assert!(
        e.contains("a super base or this the method no longer has"),
        "{e}"
    );
}

/// `for await` loops closed while `return()` awaits: left by `continue` and `break` to an
/// outer label and by `return` (the completion's kind in the frame), and by a `break`.
#[test]
fn plain_dumps_of_for_await_closes() {
    let src = r#"
async function* source(tag) {
  try { yield tag + "1"; yield tag + "2"; } finally { print(tag, "closing", await later(tag)); }
}
async function pick(mode) {
  outer: for (let i = 0; i < 2; i++) {
    inner: for await (const v of source(mode + i)) {
      print("got", v);
      if (mode === "c") continue outer;
      if (mode === "b") break outer;
      if (mode === "r") return "returned " + v;
      break inner;
    }
    print("after", i);
  }
  return mode;
}
async function main() {
  for (const m of ["c", "b", "r", "x"]) print(await pick(m));
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("closes", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("closes", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 6, "{ok}");
}

/// Destructuring stopped at every kind of place: holes and rest elements after the site,
/// an object nested in an array and an array in an object, computed keys and a rest
/// property after the site, getters read in order, reference targets of assignment
/// patterns, a source that is a string, an iterator closed after the pattern.
#[test]
fn plain_dumps_of_patterns() {
    let src = r#"
const log = [];
const src = {
  get a() { log.push("get a"); return undefined; },
  get b() { log.push("get b"); return [undefined, 2, 3, 4]; },
  c: 3, d: 4, [Symbol.for("s")]: 5,
};
function* gen() {
  try { yield 1; yield undefined; yield 3; yield 4; } finally { log.push("closed"); }
}
async function f1() {
  const { a = await later("A"), b: [x = await later("X"), , ...ys], ["c" + ""]: c, ...rest } = src;
  return [a, x, ys.join(), c, Object.keys(rest).join(), log.join()].join(" ");
}
async function f2() {
  const [p, q = await later("Q"), { r = await later("R") } = {}] = gen();
  return [p, q, r, log.join()].join(" ");
}
async function f3() {
  const t = {};
  let m;
  [t.a = await later("ta"), t[await later("k")] = await later("tk"), ...m] = "xyz".split("");
  ({ length: t.len = await later("len"), 0: t.first } = "hello");
  return JSON.stringify(t) + " " + m.join();
}
async function f4() {
  const [{ u = await later("U"), ...more } = { v: 1, w: 2 }] = [];
  return u + " " + JSON.stringify(more);
}
async function main() {
  print(await f1());
  print(await f2());
  print(await f3());
  print(await f4());
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("patterns", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("patterns", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 6, "{ok}");
}

/// A site in an assignment's member target (its object, its computed key; plain, compound
/// and logical operators, the key converted after the right side as the original does)
/// and an assignment to a pattern with the site on its right (0203 follow-ups).
#[test]
fn plain_dumps_sites_in_assignment_targets() {
    let src = r#"
async function main() {
  const o = { a: 1, n: 5 };
  o[await later("a")] = await later(2);
  (await later(o)).b = 7;
  (await later(o))[await later("c")] = 8;
  o[await later("n")] += await later(10);
  (o)[await later("p")] = 1;
  ((o))[await later("r")] ??= await later(3);
  (await later(o)).n -= await later(1);
  (await later(o)).z ||= await later(1);
  o[await later({ toString() { print("key"); return "k"; } })] = (print("rhs"), await later(1));
  ({ a: o.d = await later(3) } = { a: await later(undefined) });
  [o.e, o.f = await later(4)] = [await later(9)];
  let x, y;
  ({ x, y = await later(5) } = await later({ x: 1 }));
  print(JSON.stringify(o), x, y);
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("targets", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("targets", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 20, "{ok}");
}

/// `yield*` stopped at its yield and at the awaits of inner results: `throw` and
/// `return` forwarded after the dump, an inner iterator without `throw`, an async
/// delegation to a sync iterable whose values are promises.
#[test]
fn plain_dumps_of_yield_star() {
    let src = r#"
function* inner() { try { yield 1; yield 2; } finally { print("inner closed"); } }
function* outer() { const r = yield* inner(); yield* [3, 4]; return r; }
async function* ainner() {
  try { yield await later("a1"); yield await later("a2"); } catch (e) { print("ainner caught", e); yield "after"; }
}
async function* aouter() { yield* ainner(); yield* [later("s1"), "s2"]; }
async function main() {
  const g = outer();
  print(g.next().value);
  await later(0);
  print(JSON.stringify(g.throw("boom")) === undefined);
}
async function main2() {
  const g = outer();
  g.next(); g.next(); g.next();
  await later(1);
  try { g.throw(new Error("no throw")); } catch (e) { print("caught", e.message); }
  const a = aouter();
  print((await a.next()).value);
  await later(2);
  print(JSON.stringify(await a.throw("t")));
  for await (const v of a) print("v", v);
}
main().catch((e) => print("main", e)).then(main2);
"#;
    let (failed, refused, ok) = plain_check_program("yieldstar", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("yieldstar", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 5, "{ok}");
}

/// An async `yield*` awaiting its inner `return()`'s promise (0203 follow-up, D2887): a
/// done result returns through the delegating generator's `finally`, one that is not
/// yields and the delegation goes on.
#[test]
fn plain_dumps_of_yield_star_awaiting_return() {
    let src = r#"
async function* inner(tag) {
  try { yield tag + 1; yield tag + 2; } finally { print("inner finally", tag, await later(tag)); }
}
async function* outer(src) {
  try {
    const r = yield* src;
    print("delegated", r);
  } finally {
    print("outer finally", await later("f"));
  }
  return "end";
}
function custom() {
  let n = 0;
  return {
    [Symbol.asyncIterator]() { return this; },
    next() { return later({ value: "c" + n++, done: false }); },
    return(v) { print("custom return", v); return later(n < 3 ? { value: "kept" + n++, done: false } : { value: v + "!", done: true }); },
  };
}
async function main() {
  const g = outer(inner("a"));
  print(JSON.stringify(await g.next()));
  print(JSON.stringify(await g.return("stop")));
  print(JSON.stringify(await g.next()));
  const h = outer(custom());
  print(JSON.stringify(await h.next()));
  print(JSON.stringify(await h.return("x")));
  print(JSON.stringify(await h.return("y")));
  print(JSON.stringify(await h.next()));
  print(JSON.stringify(await h.return("z")));
  print(JSON.stringify(await h.next()));
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("yieldstar-return", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("yieldstar-return", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 8, "{ok}");
}

/// `using` scopes stopped in their code and in their disposal: a labelled jump out of a
/// scope whose async disposal awaits, disposals that throw (suppressed errors), a sync
/// `using` per iteration, a `null` resource before a sync one.
#[test]
fn plain_dumps_of_using() {
    let src = r#"
const res = (name, fail) => ({
  async [Symbol.asyncDispose]() {
    print("disposing", name, await later(name));
    if (fail) throw new Error(name);
  },
});
const sync = (name) => ({ [Symbol.dispose]() { print("dispose", name); } });
async function jumps() {
  outer: for (const i of [1, 2]) {
    {
      await using a = res("a" + i);
      using s = sync("s" + i);
      await later("in" + i);
      if (i === 1) continue outer;
      break outer;
    }
  }
  return "jumps";
}
async function errors() {
  try {
    await using x = res("x", true);
    await using y = res("y", true);
    throw new Error("body");
  } catch (e) {
    return [e.message, e.error && e.error.message, e.suppressed && e.suppressed.message].join();
  }
}
async function iter() {
  for (using r of [sync("r1"), sync("r2")]) print(await later("it"));
  {
    await using n = null;
    using t = sync("t");
    await later("t");
  }
  return "iter";
}
async function main() { print(await jumps()); print(await errors()); print(await iter()); }
main();
"#;
    let (failed, refused, ok) = plain_check_program("using", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("using", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 8, "{ok}");
}

/// Iterator helpers held across awaits at every state: a flatMap inside its inner
/// iterator, a concat inside an item, take and drop counting down, a filter's and a
/// map's counter, a helper closed by `return`, Iterator.from's wrapper, and chains of
/// re-dumps through them.
#[test]
fn plain_dumps_of_iterator_helpers() {
    let src = r#"
function* nums() { try { let i = 0; while (true) yield i++; } finally { print("nums closed"); } }
async function main() {
  const fm = [[1, 2, 3], [4]].values().flatMap((a, i) => a.map((x) => x * 10 + i));
  const a = [fm.next().value];
  await later("fm1");
  a.push(fm.next().value);
  await later("fm2");
  a.push(...fm);
  print("flatMap", a.join());
  const cat = Iterator.concat(["p", "q", "r"], new Set(["s"]), ["t"]);
  const c = [cat.next().value];
  await later("cat");
  c.push(cat.next().value, cat.next().value, cat.next().value);
  await later("cat2");
  c.push(...cat);
  print("concat", c.join());
  const chain = nums().filter((n, i) => (n + i) % 4 === 0).map((n, i) => n + ":" + i).drop(1).take(3);
  const d = [];
  for (const v of chain) d.push(v, await later(v));
  print("chain", d.join());
  const closing = nums().map((n) => n * 2);
  closing.next();
  await later("close");
  print("return", JSON.stringify(closing.return()), JSON.stringify(closing.next()));
  const w = Iterator.from({ i: 0, next() { return { value: this.i++, done: this.i > 3 }; } });
  w.next();
  await later("from");
  print("from", [...w].join());
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("helpers", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("helpers", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 9, "{ok}");
}

/// Sites in loop heads and tests, `var` declarations and `for (var ...)` loops, object
/// literals (a computed key, methods and accessors after the site), a logical assignment
/// to a member, a tagged template, an update of a member, a catch parameter (D2880).
#[test]
fn plain_dumps_of_heads_and_more() {
    let src = r#"
async function varDecl() { var v = await later(1); var w = v + (await later(2)); return v + w; }
async function loopTest() { let n = 0; while (await later(n < 2)) n++; return n; }
async function forHead() { let s = 0; for (let i = await later(0); i < 2; i++) s += i; return s; }
async function forOfHead() { let s = ""; for (const x of await later(["a", "b"])) s += x; return s; }
async function forVar() { let s = 0; for (var i = 0; i < 2; i++) s += await later(i); return s + i; }
async function forVarOf() { let s = ""; for (var x of ["p", "q"]) s += await later(x); return s + x; }
async function computedKey() { return JSON.stringify({ a: 1, [await later("k")]: 2 }); }
async function methodAfter() { const o = { a: await later(1), m() { return 2; }, get g() { return 3; } }; return o.a + o.m() + o.g; }
async function logicalMember() { const o = { p: null }; o.p ??= await later(4); return o.p; }
async function tagged() { const t = (s, ...v) => s.raw.join("|") + v.join(); return t`a${await later(1)}b${2}`; }
async function update() { const o = { k: 1 }; o[await later("k")]++; return o.k; }
async function catchParam() { try { throw {}; } catch ({ m = await later("m") }) { return m; } }
async function main() {
  for (const f of [varDecl, loopTest, forHead, forOfHead, forVar, forVarOf, computedKey, methodAfter, logicalMember, tagged, update, catchParam]) {
    print(f.name, await f());
  }
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("heads", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("heads", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 17, "{ok}");
}

/// A program read from `DJS_PROBE`, dumped at every idle point and chained: what is
/// refused or differs (a tool for finding the constructs plain dumps miss).
#[test]
#[ignore]
fn plain_dumps_probe() {
    let src = std::fs::read_to_string(std::env::var("DJS_PROBE").unwrap()).unwrap();
    let (failed, refused, ok) = plain_check_program("probe", &src);
    std::println!(
        "ok {ok}\nrefused {refused:#?}\nfailed {}",
        failed.join("\n")
    );
    let (failed, links, refused) = plain_chain_program("probe", &src);
    std::println!(
        "chain links {links} refused {refused:#?}\nfailed {}",
        failed.join("\n")
    );
}

/// Classes whose methods read variables a waiting frame shares: their statements go into
/// the continuation's block, a subclass after its parent (D2881).
#[test]
fn plain_dumps_of_shared_classes() {
    let src = r#"
async function classShared() {
  let count = 0;
  const tag = "t";
  class C { inc() { return ++count; } static make() { return new C(); } }
  class D extends C { inc() { return super.inc() * 10 + tag.length; } }
  const c = C.make();
  const d = new D();
  c.inc();
  await later("cls");
  const r = [c.inc(), d.inc(), count, d instanceof C];
  await later("cls2");
  return r.concat(c.inc()).join();
}
classShared().then(print);
"#;
    let (failed, refused, ok) = plain_check_program("classes", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("classes", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 2, "{ok}");
}

/// Classes that name themselves (a class expression on a global, a class in a waiting
/// frame) and a class whose static private field is still at its initial value (D2883).
#[test]
fn plain_dumps_of_self_naming_classes() {
    let src = r#"
globalThis.A = class A { static n = 0; m() { return A; } };
A.n++;
class B { static #o = 0; static o() { return B.#o++; } }
globalThis.B = B;
async function main() {
  class C { static n = 0; constructor() { C.n++; } m() { return C; } }
  const c = new C();
  await later(1);
  print(A.n, new A().m() === A, B.o(), B.o(), C.n, c.m() === C);
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("self-classes", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("self-classes", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 1, "{ok}");
}

/// Re-dumps pjz found refused (0207): a function declared in a loop body, an `await
/// using` block's disposal after copied text, an await in a `finally` in a loop's later
/// iteration, a disposal whose block has a function sharing a `let` (D2884).
#[test]
fn plain_redumps_of_loops_finally_and_using() {
    let src = r#"
async function f6() {
  for (const x of [1, 2]) {
    async function g() { return await later(x); }
    print(await g());
  }
}


async function f7() {
  await later(0);
  { await using r = { async [Symbol.asyncDispose]() { print("bye", await later(1)); } }; }
  print("after");
}


async function g() { for (const x of [1, 2]) { try {} finally { print(await later(x)); } } }


async function h() {
  { await using r = { async [Symbol.asyncDispose]() { print("bye", await later(2)); } }; let n = 1; function f(y) { return n * 2 + y; } print(f(1)); }
}

async function main() { await f6(); await f7(); await g(); await h(); }
main();
"#;
    let (failed, refused, ok) = plain_check_program("redumps", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("redumps", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 6, "{ok}");
}

/// Re-dumps of a continuation stopped again inside its own rewritten statement: a binary
/// operator's left operand, a chain of them, a template's substitution, a callee whose key
/// awaited, an assignment whose value awaits twice (D2885).
#[test]
fn plain_redumps_inside_rewritten_statements() {
    let src = r#"
async function t3() { const x = (await later(1)) + (await later(2)); const o = {}; o.a = (await later(3)) + (await later(4)); print(x, o.a, (await later(5)) + (await later(6))); }
async function t4() { print(1 + (await later(1)) + (await later(2)) + (await later(3))); }
async function t5() { print(`${await later("a")}-${await later("b")}`); }
async function t8() { const fs = [(v) => "f" + v]; print(fs[await later(0)](await later(2))); }
async function t9() { const f = async (v) => { let s = 0; for (const i of [1, 2]) s += await later(i); return s + v; }; const o = {}; o["k"] = await f(await later(1)); print(o.k); }
async function main() { await t3(); await t4(); await t5(); await t8(); await t9(); }
main();
"#;
    let (failed, refused, ok) = plain_check_program("rewritten", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("rewritten", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 10, "{ok}");
}

// 0207's tests (plain dump size), in a file of their own.
#[path = "dump_size_tests.rs"]
mod size;

/// Closures of a suspended frame that data reaches (a tools object holding one) and that
/// read each other: the variables read them where their scope objects put them, not
/// nothing (0215; found by the dump benchmark's fake tools).
#[test]
fn plain_dumps_of_frame_closures_reached_by_data() {
    let src = r#"
async function main() {
  let seed = 5;
  const rnd = (n) => { seed ^= seed << 3; seed >>>= 0; return seed % n; };
  const sentence = (n) => [rnd(n), rnd(n)].join(" ");
  const tools = { a: () => later(sentence(5)) };
  return await (async (tools) => {
    print(await tools.a());
    print(await tools.a());
  })(tools);
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("frame-closures", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, _, refused) = plain_chain_program("frame-closures", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 2, "{ok}");
}

/// Re-dumps inside loops K copied as the original's text (a later iteration of an outer
/// loop): the inner loop's iterator is the copy's, not the stale constant of K's
/// scaffold for the first iteration (0215; pjz's nested for-in).
#[test]
fn plain_dumps_of_loops_in_copied_text() {
    let src = r#"
async function* gen(n) { for (let i = 0; i < n; i++) yield i; }
async function main() {
  let count = 0;
  for (const a of [1, 2]) for (const b in a === 1 ? { c: 3 } : { a: 1, b: 2, c: 3 }) { print(await later(b)); count++; }
  for (const a of [1, 2, 3]) for (const b of a === 1 ? ["x"] : ["y", "z", "w"]) { print(a, await later(b)); count++; }
  for (const a of [1, 2]) for await (const b of gen(a + 1)) { print(a, await later(b)); count++; }
  for (const a of [1, 2]) { try { print(await later(a)); } finally { print("fin", await later(a)); count++; } }
  outer: for (const a of [1, 2]) for (const b of [1, 2, 3]) { if (b === 3) continue outer; print(a, b, await later(b)); }
  for (const a of [1, 2]) { const [x, y = await later(a)] = [a]; print(x, y); }
  print(count);
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("copied-loops", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, links, refused) = plain_chain_program("copied-loops", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 26 && links >= 26, "{ok} {links}");
}

/// A `continue` or `break` of a `for await` pending in an `await using` scope's disposal
/// in the rest of the iteration K runs in its loop-once scaffold: re-dumps name the
/// original's loop (pjz, 0211: "no value for completion.kind").
#[test]
fn plain_dumps_of_jumps_pending_in_disposals_of_for_await() {
    let src = r#"
async function* g(k) { for (let i = 0; i < k; i++) yield await later(i); }
const res = (n) => ({ async [Symbol.asyncDispose]() { print("bye", n, await later(n)); } });
async function main() {
  for await (const x of g(2)) {
    try { await later("x"); throw new Error("ko"); } catch (e) { print(e.message); }
    { await using r = res("c" + x); if (true) continue; }
  }
  for await (const x of g(3)) {
    await later(x);
    { await using r = res("b" + x); if (x === 1) break; }
  }
  outer: for (const a of [1, 2]) {
    for await (const x of g(2)) {
      await later(x);
      { await using r = res("o" + a + x); if (x === 0) continue outer; }
    }
  }
  print("end");
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("jump-disposal", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, links, refused) = plain_chain_program("jump-disposal", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 18 && links >= 18, "{ok} {links}");
}

/// Bindings not declared yet in a later loop iteration, whose locals still hold the
/// earlier iteration's values: written as in their TDZ, their old values left out
/// (0215); a declaration whose pattern holds the site keeps what it has bound.
#[test]
fn plain_dumps_leave_out_stale_locals() {
    let src = r#"
async function main() {
  for (const i of [0, 1]) for (let j = 0; j < 1; j++) {
    let n = i;
    class C { get(x) { return x + n; } async fetch(x) { return await later(x); } }
    const o = new C(i > 0 ? await later(4) : 4); print(o.get(1));
    const p = { v: i }, q = i > 0 ? await later(p.v) : 0, { r } = { r: i > 0 ? await later(5) : 5 };
    print(p.v, q, r);
    let late = { i };
    print(late.i);
  }
  for (const k of [0, 1, 2]) {
    if (k > 0) print("before", await later(k));
    const obj = { k, f() { return k; } };
    print(obj.f());
  }
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("stale", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, links, refused) = plain_chain_program("stale", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 5 && links >= 5, "{ok} {links}");
}

/// Array.fromAsync waiting on an await (an async iterator's step, a mapped value, an
/// array-like's element), dumped as the runtime's `fromAsync` and re-dumped from it
/// (0215, pjz: "an internal function of a builtin").
#[test]
fn plain_dumps_of_array_from_async() {
    let src = r#"
async function* g(n) { for (let i = 0; i < n; i++) yield await later(i); }
async function main() {
  print(JSON.stringify(await Array.fromAsync(g(2))));
  print(JSON.stringify(await Array.fromAsync([later("a"), "b"], async (x, i) => x + i + await later(i))));
  print(JSON.stringify(await Array.fromAsync({ length: 2, 0: later("x"), 1: "y" })));
}
main();
"#;
    let (failed, refused, ok) = plain_check_program("from-async", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    let (failed, links, refused) = plain_chain_program("from-async", src);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(ok >= 6 && links >= 6, "{ok} {links}");
}
