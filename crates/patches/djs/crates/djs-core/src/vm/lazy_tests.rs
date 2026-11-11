//! Lazy functions (D2990): results as eager, every early error at load, and the same
//! code whether a function compiles with its unit or at its first call.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::bytecode::template::{bytes_of, code, debug_info};
use crate::compiler::CompileOptions;
use crate::heap::CellKind;
use crate::heap::cells::{FuncInfo, FuncTemplate, U32Array};

/// A VM from a base image made once per test thread (`Vm::new` compiles the prelude).
fn vm(lazy: bool) -> Vm {
    extern crate std;
    std::thread_local! {
        static BASE: Vec<u8> = {
            let mut vm = Vm::new(VmOptions::default()).unwrap();
            vm.prepare_base_image().unwrap();
            vm.bytes().to_vec()
        };
    }
    let opts = VmOptions {
        lazy_functions: lazy,
        ..VmOptions::default()
    };
    BASE.with(|b| Vm::from_base(b, opts)).unwrap()
}

/// The completion value of `src` as a script (`throw ...`, `syntax ...`).
fn eval_with(vm: &mut Vm, src: &str, opts: &CompileOptions<'_>) -> String {
    let f = match vm.compile(src.as_bytes(), opts) {
        Ok(f) => f,
        Err(e) => return alloc::format!("syntax {} at {}:{}", e.message, e.line, e.col),
    };
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    match vm.run(u64::MAX) {
        Status::Done(v) => vm.display(v),
        Status::Threw(v) => alloc::format!("throw {}", vm.display(v)),
        s => panic!("{s:?}"),
    }
}

fn both(src: &str) -> String {
    let opts = CompileOptions::script("t.js");
    let eager = eval_with(&mut vm(false), src, &opts);
    let lazy = eval_with(&mut vm(true), src, &opts);
    assert_eq!(lazy, eager, "{src}");
    lazy
}

#[test]
fn results_are_the_same() {
    let cases = [
        "function f(a, b = 2) { return a + b } f(1)",
        "let x = 1; const g = () => x++; g(); g(); x",
        "function outer() { let n = 0; return { inc() { return ++n }, get v() { return n } } } \
         const o = outer(); o.inc(); o.inc(); o.v",
        "function* gen() { yield 1; yield 2 } [...gen()].join()",
        "async function af() { return 5 } let r; af().then(v => r = v); r",
        "class A { #p = 3; m() { return this.#p } static s() { return 'S' } } new A().m() + A.s()",
        "class B { m() { return 'b' } } class C extends B { m() { return () => super.m() + 'c' } } new C().m()()",
        "function named() { return typeof named } named()",
        "const h = function self(n) { return n ? self(n - 1) + 1 : 0 }; h(3)",
        "function a1() { return arguments.length + (() => arguments[0])() } a1(10, 2)",
        "function nt() { return (() => new.target === nt)() } new nt() instanceof nt",
        "(function () { return 'iife' })()",
        "function tdz() { const f = () => q; try { f() } catch (e) { return e.constructor.name } let q = 1 } tdz()",
        "function ro() { const c = 1; const f = () => { c = 2 }; try { f() } catch (e) { return e.constructor.name } } ro()",
        "function deep() { function a() { function b() { return 'deep' } return b() } return a() } deep()",
        "function st() { try { null.x } catch (e) { return e.stack.split('\\n')[1] } } st()",
        "function ts() { return 1 } ts.toString() + ts.length + ts.name",
        "const ff = (a, b) => a * b; [1, 2, 3].map(x => ff(x, 2)).join()",
        "function cl() { let i = 0; const fs = []; for (let j = 0; j < 3; j++) fs.push(() => j + i); i = 10; return fs.map(f => f()).join() } cl()",
        "label: for (const k of [1]) { (() => k)(); break label } 'ok'",
    ];
    for c in cases {
        both(c);
    }
}

#[test]
fn early_errors_are_reported_at_load() {
    let r = both("function never() { let a; let a; } 1");
    assert!(r.starts_with("syntax "), "{r}");
    let r = both("function never() { return () => { break; } } 1");
    assert!(r.starts_with("syntax "), "{r}");
    let r = both("class K { m() { return this.#nope } } 1");
    assert!(r.starts_with("syntax "), "{r}");
}

/// Every lazy template that ran is compiled to what an eager compile makes of it.
fn templates(vm: &Vm) -> Vec<(u32, u32, u32)> {
    let h = &vm.heap;
    h.cells()
        .filter(|c| c.kind == CellKind::FuncTemplate)
        .filter_map(|c| {
            let d = debug_info(h, c.offset);
            d.script.get()?;
            Some((d.src_start, d.src_end, c.offset))
        })
        .collect()
}

/// A template's parts that must be equal: code, constants, line table, names, sizes.
fn shape(vm: &Vm, t: u32) -> String {
    let h = &vm.heap;
    let a = h.arena();
    let d = debug_info(h, t);
    let names: Vec<u32> = match d.names.get() {
        Some(n) => (0..a.get::<u32>(n, U32Array::LEN))
            .map(|i| a.read(U32Array::DATA.at(n, i)))
            .collect(),
        None => Vec::new(),
    };
    let consts: Vec<String> = match a.get::<Ref>(t, FuncTemplate::CONSTS).get() {
        Some(p) => (0..h.value_array_len(p))
            .map(|i| {
                let v = h.value_array_get(p, i);
                match v.as_string() {
                    Some(s) => h.str_to_string(s),
                    None if v.as_number().is_some() => alloc::format!("{:x}", v.to_bits()),
                    None => String::from("cell"),
                }
            })
            .collect(),
        None => Vec::new(),
    };
    let info: Ref = a.get(t, FuncTemplate::INFO);
    let children = info
        .get()
        .map_or(0, |i| a.get::<u32>(i, FuncInfo::CHILD_COUNT));
    alloc::format!(
        "code {:?} consts {:?} lines {:?} names {} flags {} locals {} stack {} children {}",
        code(h, t),
        consts,
        bytes_of(h, d.lines),
        names.len(),
        a.get::<u16>(t, FuncTemplate::FLAGS),
        a.get::<u16>(t, FuncTemplate::LOCALS),
        a.get::<u16>(t, FuncTemplate::STACK),
        children,
    )
}

/// Compiles every lazy template of `vm` (what `Vm::seal` does) and checks each against
/// the template of the same span in an eager compile of `src`.
fn check_identity(src: &str, opts: &CompileOptions<'_>) -> usize {
    let mut lazy = vm(true);
    let Ok(_) = lazy.compile(src.as_bytes(), opts) else {
        return 0;
    };
    lazy.compile_lazy_templates().unwrap();
    let mut eager = vm(false);
    eager.compile(src.as_bytes(), opts).unwrap();
    let eager_t = templates(&eager);
    let mut n = 0;
    for (s, e, t) in templates(&lazy) {
        let c: Ref = lazy.heap.arena().get(t, FuncTemplate::IC);
        let Some(c) = c.get() else { continue };
        if code(&lazy.heap, t) != [crate::bytecode::Op::Lazy as u8] {
            continue;
        }
        let Some(&(_, _, et)) = eager_t.iter().find(|x| x.0 == s && x.1 == e) else {
            panic!("no eager template for {s}..{e}");
        };
        assert_eq!(shape(&lazy, c), shape(&eager, et), "function at {s}..{e}");
        n += 1;
    }
    n
}

#[test]
fn lazy_code_is_eager_code_corpus() {
    extern crate std;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let mut files = Vec::new();
    let mut dirs = alloc::vec![root];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "js" || x == "ts") {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut n = 0;
    for p in files {
        let src = std::fs::read_to_string(&p).unwrap();
        let ts = p.extension().is_some_and(|x| x == "ts");
        let mut opts = CompileOptions::async_body("t.js", &[]);
        opts.typescript = ts;
        n += check_identity(&src, &opts);
    }
    // Plain dumps, as Scripts.
    let golden =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/dump/v2/golden");
    for e in std::fs::read_dir(golden).unwrap() {
        let p = e.unwrap().path();
        if p.to_string_lossy().ends_with(".dump.js") {
            let src = std::fs::read_to_string(&p).unwrap();
            n += check_identity(&src, &CompileOptions::script("dump.js"));
        }
    }
    assert!(n > 1000, "{n} lazy functions compared");
}

/// `Vm::seal` leaves no lazy template uncompiled, and the closures still run.
#[test]
fn seal_compiles_lazy_templates() {
    let mut vm = vm(true);
    let src =
        "function later(n) { const sq = x => x * x; return sq(n) + 1 } globalThis.later = later; 0";
    assert_eq!(
        eval_with(&mut vm, src, &CompileOptions::script("t.js")),
        "0"
    );
    let pending = |vm: &Vm| {
        templates(vm)
            .iter()
            .filter(|&&(_, _, t)| {
                code(&vm.heap, t) == [crate::bytecode::Op::Lazy as u8]
                    && vm
                        .heap
                        .arena()
                        .get::<Ref>(t, FuncTemplate::IC)
                        .get()
                        .is_none()
            })
            .count()
    };
    assert_eq!(pending(&vm), 1);
    vm.seal().unwrap();
    assert_eq!(pending(&vm), 0);
    let r = eval_with(&mut vm, "later(4)", &CompileOptions::script("u.js"));
    assert_eq!(r, "17");
}

/// The maps of a lazy load (the top unit's) and of each lazy function on demand equal
/// the eager compile's maps of the same functions (D2994).
#[cfg(feature = "dump")]
fn check_maps(src: &str, opts: &CompileOptions<'_>) -> usize {
    use crate::heap::Heap;
    let mut eh = Heap::new().unwrap();
    let Ok(eager) = crate::compiler::suspension_maps(&mut eh, src.as_bytes(), opts) else {
        return 0;
    };
    let mut lh = Heap::new().unwrap();
    let (_, mut maps, _) =
        crate::compiler::compile_lazy_with_maps(&mut lh, src.as_bytes(), opts).unwrap();
    // Every lazy template, nested ones too: compile each on its own (as `Vm` would at a
    // first call) and collect its maps.
    let mut done = Vec::new();
    loop {
        let todo: Vec<u32> = lh
            .cells()
            .filter(|c| c.kind == CellKind::FuncTemplate)
            .map(|c| c.offset)
            .filter(|&t| code(&lh, t) == [crate::bytecode::Op::Lazy as u8] && !done.contains(&t))
            .collect();
        if todo.is_empty() {
            break;
        }
        for t in todo {
            done.push(t);
            let mut scratch = Heap::new().unwrap();
            maps.extend(crate::compiler::lazy_function_maps(&lh, Ref(t), &mut scratch).unwrap());
            // And compile it here, so that its lazy children appear.
            let c = crate::compiler::compile_lazy(&mut lh, Ref(t)).unwrap();
            lh.arena_mut().set(t, FuncTemplate::IC, c);
        }
    }
    let mut n = 0;
    for e in &eager {
        let Some(m) = maps.iter().find(|m| m.start == e.start && m.end == e.end) else {
            panic!("no lazy map for {}..{}", e.start, e.end);
        };
        assert_eq!(m.text(src.as_bytes()), e.text(src.as_bytes()));
        n += 1;
    }
    n
}

#[cfg(feature = "dump")]
#[test]
fn lazy_maps_are_eager_maps() {
    extern crate std;
    let mut n = 0;
    for area in ["codemode", "codemode-harvest", "language"] {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus")
            .join(area);
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "js") {
                let src = std::fs::read_to_string(&p).unwrap();
                n += check_maps(&src, &CompileOptions::async_body("t.js", &[]));
            }
        }
    }
    let golden =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/dump/v2/golden");
    for e in std::fs::read_dir(golden).unwrap() {
        let p = e.unwrap().path();
        if p.to_string_lossy().ends_with(".dump.js") {
            let src = std::fs::read_to_string(&p).unwrap();
            n += check_maps(&src, &CompileOptions::script("dump.js"));
        }
    }
    assert!(n > 300, "{n} maps compared");
}

#[test]
#[ignore]
fn probe_compile_cells() {
    extern crate std;
    let mut vm = vm(false);
    let src = std::fs::read_to_string(std::env::var("PROBE").unwrap()).unwrap();
    let before = vm.heap.arena().len();
    let f = vm
        .compile(
            src.as_bytes(),
            &CompileOptions::async_body("codemode.js", &["tools"]),
        )
        .unwrap();
    let after = vm.heap.arena().len();
    let mut by: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
    for c in vm.heap.cells().filter(|c| c.offset >= before) {
        let e = by.entry(alloc::format!("{:?}", c.kind)).or_default();
        e.0 += 1;
        e.1 += vm.heap.cell_size(c.offset);
    }
    std::println!("src {} bytes, arena +{} ", src.len(), after - before);
    for (k, (n, b)) in by {
        std::println!("{k}: {n} cells {b} bytes");
    }
    let _ = f;
}
