//! `debugger;` as a pause (D3050): with `CompileOptions::debugger_awaits`, the statement
//! in an async function awaits a call of the host's async function `debugger`, so the
//! run is idle there and can be dumped. The dump writes the pause as any open host call;
//! loaded with the option, a continuation pauses again at the next `debugger;`.
//!
//! Each program runs once without dumps (every pause settled at once) and once with a
//! dump and a load into a fresh VM at every pause, the loaded run being dumped again at
//! the next pause (a re-dump): both must print the same lines.

extern crate std;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::*;

const DEBUGGER: u32 = 1;
const PRINT: u32 = 2;

fn base() -> &'static [u8] {
    static BASE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        let mut v = Vm::new(VmOptions::default()).unwrap();
        v.prepare_base_image().unwrap();
        v.bytes().to_vec()
    })
}

fn new_vm() -> Vm {
    let mut vm = Vm::from_base(
        base(),
        VmOptions {
            frozen_builtins: true,
            ..VmOptions::default()
        },
    )
    .unwrap();
    let d = vm.new_async_host_function(DEBUGGER, "debugger").unwrap();
    vm.define_global("debugger", d).unwrap();
    let p = vm.new_host_function(PRINT, "print").unwrap();
    vm.define_global("print", p).unwrap();
    vm
}

fn host_fn(name: &str) -> Option<u32> {
    match name {
        "debugger" => Some(DEBUGGER),
        "print" => Some(PRINT),
        _ => None,
    }
}

fn host_fn_name(id: u32) -> Option<String> {
    match id {
        DEBUGGER => Some("debugger".into()),
        PRINT => Some("print".into()),
        _ => None,
    }
}

fn opts() -> crate::compiler::CompileOptions<'static> {
    crate::compiler::CompileOptions {
        debugger_awaits: true,
        ..crate::compiler::CompileOptions::script("src.js")
    }
}

fn print(vm: &mut Vm, lines: &mut Vec<String>, call_id: u32) {
    let args = vm.host_call(call_id).unwrap().args;
    let parts: Vec<String> = args.iter().map(|&v| vm.display(v)).collect();
    lines.push(parts.join(" "));
    vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
}

/// Runs until the VM is idle: the open `debugger` call, if any.
fn step(vm: &mut Vm, lines: &mut Vec<String>) -> Option<u32> {
    loop {
        match vm.run(1_000_000_000) {
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, PRINT);
                print(vm, lines, call_id);
            }
            Status::Idle | Status::Done(_) => {
                let open = vm.pending_host_calls();
                assert!(open.len() <= 1, "{open:?}");
                return open.first().copied();
            }
            Status::Threw(e) => {
                lines.push(alloc::format!("threw {}", vm.display(e)));
                return None;
            }
            s => panic!("{s:?}"),
        }
    }
}

fn start(src: &str) -> Vm {
    let mut vm = new_vm();
    let f = vm
        .compile(src.as_bytes(), &opts())
        .unwrap_or_else(|e| panic!("{} at {}:{}", e.message, e.line, e.col));
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    vm
}

/// The run with every pause settled at once: its lines and the number of pauses.
fn straight(src: &str) -> (Vec<String>, usize) {
    let mut vm = start(src);
    let (mut lines, mut pauses) = (Vec::new(), 0);
    while let Some(id) = step(&mut vm, &mut lines) {
        pauses += 1;
        vm.resolve_host_call(id, Ok(Value::UNDEFINED)).unwrap();
    }
    (lines, pauses)
}

fn dump(vm: &Vm) -> String {
    let opts = super::dump::DumpOptions {
        base: &[],
        host: &["debugger", "print"],
        host_fn_name: &host_fn_name,
        base_names: Some(super::dump_harvest::base_names()),
        ..Default::default()
    };
    vm.dump(&opts, None)
        .unwrap_or_else(|e| panic!("refused: {}", e.message()))
}

/// The run dumped at every pause and loaded into a fresh VM: its lines and dumps.
fn dumped(src: &str) -> (Vec<String>, Vec<String>) {
    let mut vm = start(src);
    let (mut lines, mut dumps) = (Vec::new(), Vec::new());
    while step(&mut vm, &mut lines).is_some() {
        let text = dump(&vm);
        let mut next = new_vm();
        let mut serve = |vm: &mut Vm, call_id: u32, fn_id: u32| {
            assert_eq!(fn_id, PRINT);
            print(vm, &mut lines, call_id);
        };
        super::dump_tests::run_plain_dump_with(&mut next, &text, &host_fn, &mut serve, true)
            .unwrap_or_else(|e| panic!("{e}\n{text}"));
        // The reopened pause: settle it, and the run goes on to the next one.
        let open = next.pending_host_calls();
        assert_eq!(open.len(), 1, "{text}");
        assert_eq!(next.host_call(open[0]).unwrap().fn_id, DEBUGGER);
        next.resolve_host_call(open[0], Ok(Value::UNDEFINED))
            .unwrap();
        dumps.push(text);
        vm = next;
    }
    (lines, dumps)
}

fn check(src: &str, pauses: usize) -> Vec<String> {
    let (want, n) = straight(src);
    assert_eq!(n, pauses, "{want:?}");
    let (got, dumps) = dumped(src);
    assert_eq!(got, want, "{dumps:#?}");
    dumps
}

#[test]
fn a_pause_in_a_loop_resumes_twice() {
    let dumps = check(
        "(async () => {\n  let sum = 0;\n  for (let i = 0; i < 3; i++) {\n    sum += i;\n    \
         print('i', i, sum);\n    debugger;\n  }\n  print('end', sum);\n})();\n",
        3,
    );
    for d in &dumps {
        assert!(d.contains("$djs.resume("), "{d}");
        assert!(d.contains("\"debugger\""), "{d}");
    }
}

#[test]
fn a_pause_in_a_nested_async_function() {
    check(
        "async function inner(x) {\n  print('in', x);\n  debugger;\n  return x * 2;\n}\n\
         (async () => {\n  const a = await inner(1);\n  const b = await inner(a);\n  print(a, b);\n})();\n",
        2,
    );
}

#[test]
fn debugger_elsewhere_does_nothing() {
    // A sync function, and the Script's own top level: no pause.
    check(
        "function f() { debugger; return 1; }\ndebugger;\n\
         (async () => { print(f()); debugger; print(2); })();\n",
        1,
    );
    // Without the option, `debugger;` in an async function does nothing either.
    let mut vm = new_vm();
    let f = vm
        .compile(
            b"(async () => { debugger; print(1); })();",
            &crate::compiler::CompileOptions::script("src.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut lines = Vec::new();
    assert_eq!(step(&mut vm, &mut lines), None);
    assert_eq!(lines, ["1"]);
}

/// Prints the dumps of a program: `DJS_SHOW=1 cargo test ... show_debugger_dumps -- --nocapture`.
#[test]
fn show_debugger_dumps() {
    if std::env::var("DJS_SHOW").is_err() {
        return;
    }
    let (_, dumps) = dumped(
        "(async () => {\n  let sum = 0;\n  for (let i = 0; i < 3; i++) {\n    sum += i;\n    \
         print('i', i, sum);\n    debugger;\n  }\n  print('end', sum);\n})();\n",
    );
    for d in dumps {
        std::println!("{d}\n-----");
    }
}

/// Runs until the VM is idle: the open calls.
fn step_all(vm: &mut Vm, lines: &mut Vec<String>) -> Vec<u32> {
    loop {
        match vm.run(1_000_000_000) {
            Status::HostCall { call_id, .. } => print(vm, lines, call_id),
            Status::Idle | Status::Done(_) => return vm.pending_host_calls(),
            s => panic!("{s:?}"),
        }
    }
}

/// `Vm::call_site` (D3054): a pause's position, in the script the run started with, then
/// in the dump each later part continued from, at the `debugger;` it stopped at; pauses
/// inside a `Promise.all` each at their own.
#[test]
fn call_sites_of_pauses() {
    let src = "(async () => {\n  let sum = 0;\n  for (let i = 0; i < 2; i++) {\n    sum += i;\n    \
               debugger;\n  }\n  await Promise.all([1, 2].map(async (x) => {\n    debugger;\n  }));\n})();\n";
    let mut vm = start(src);
    let mut lines = Vec::new();
    let open = step_all(&mut vm, &mut lines);
    let at = vm.call_site(open[0]).unwrap();
    assert_eq!((at.file.as_str(), at.line, at.col), ("src.js", 5, 5));
    // Not an open call.
    assert_eq!(vm.call_site(open[0] + 100), None);
    let mut sites = Vec::new();
    loop {
        let text = dump(&vm);
        let mut next = new_vm();
        let mut serve = |vm: &mut Vm, call_id: u32, _: u32| print(vm, &mut lines, call_id);
        super::dump_tests::run_plain_dump_with(&mut next, &text, &host_fn, &mut serve, true)
            .unwrap_or_else(|e| panic!("{e}\n{text}"));
        let line_of = |at: &CallSite| {
            text.lines()
                .nth(at.line as usize - 1)
                .unwrap_or("")
                .to_string()
        };
        // The reopened pauses wait at the dump's own `await $djs.resume(...)`.
        for id in next.pending_host_calls() {
            let at = next.call_site(id).unwrap();
            assert_eq!(at.file, "dump.js");
            assert!(line_of(&at).contains("$djs.resume("), "{at:?}\n{text}");
            next.resolve_host_call(id, Ok(Value::UNDEFINED)).unwrap();
        }
        let open = step_all(&mut next, &mut lines);
        if open.is_empty() {
            break;
        }
        let mut here = Vec::new();
        for &id in &open {
            let at = next.call_site(id).unwrap();
            assert_eq!(at.file, "dump.js");
            let line = line_of(&at);
            assert_eq!(&line[at.col as usize - 1..], "debugger;", "{at:?}\n{text}");
            here.push(at.line);
        }
        sites.push(here);
        vm = next;
    }
    // The loop's second pause, then the two callbacks' pauses at once, on one line.
    assert_eq!(sites.len(), 2, "{sites:?}");
    assert_eq!(sites[0].len(), 1);
    assert_eq!(sites[1].len(), 2);
    assert_eq!(sites[1][0], sites[1][1]);
}
