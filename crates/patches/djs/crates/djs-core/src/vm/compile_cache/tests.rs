//! A cache hit leaves the VM as the compile it replaces would have (D3040).

use alloc::string::String;
use alloc::vec::Vec;

use super::super::*;
use crate::compiler::CompileOptions;

/// A VM from a base image made once per test thread.
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
        #[cfg(feature = "lazy")]
        lazy_functions: lazy,
        ..VmOptions::default()
    };
    let _ = lazy;
    BASE.with(|b| Vm::from_base(b, opts)).unwrap()
}

fn run(vm: &mut Vm, f: Value) -> String {
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    match vm.run(u64::MAX) {
        Status::Done(v) => vm.display(v),
        Status::Threw(v) => alloc::format!("throw {}", vm.display(v)),
        Status::Idle => String::from("idle"),
        s => alloc::format!("{s:?}"),
    }
}

/// Compiles `src` cold in one VM, imports its blob into another VM in the same state
/// (after `prep` ran in both), and compares both heaps byte for byte, before and after
/// running the function.
fn check(src: &str, opts: &CompileOptions<'_>, lazy: bool, prep: &str) {
    let po = CompileOptions::script("prep.js");
    let prepare = |vm: &mut Vm| {
        if !prep.is_empty() {
            let f = vm.compile(prep.as_bytes(), &po).unwrap();
            run(vm, f);
        }
    };
    let mut a = vm(lazy);
    let mut b = vm(lazy);
    // The blob comes from a VM that did not run `prep`: another state.
    let mut c = vm(lazy);
    prepare(&mut a);
    prepare(&mut b);
    let (_, blob) = c.compile_cached(src.as_bytes(), opts, None).unwrap();
    let blob = blob.expect("a blob");
    let fa = a.compile(src.as_bytes(), opts).unwrap();
    let (fb, again) = b.compile_cached(src.as_bytes(), opts, Some(&blob)).unwrap();
    assert!(again.is_none(), "the blob was not taken");
    assert!(
        a.heap.arena().bytes() == b.heap.arena().bytes(),
        "heaps differ after the import: {src:.80}"
    );
    let (ra, rb) = (run(&mut a, fa), run(&mut b, fb));
    assert_eq!(ra, rb);
    assert!(
        a.heap.arena().bytes() == b.heap.arena().bytes(),
        "heaps differ after the run: {src:.80}"
    );
}

#[test]
fn a_hit_is_the_compile() {
    let opts = CompileOptions::script("t.js");
    for lazy in [false, true] {
        for prep in [
            "",
            "globalThis.zork = { quux: 1, 'q-z': [2] }; var later = 3",
        ] {
            check("function f(a) { return a + later } f(1)", &opts, lazy, prep);
            check(
                "let x = { zork: 1, 'new key': 2 }; Object.keys(x).join() + zork.quux",
                &opts,
                lazy,
                prep,
            );
            check(
                "class A { #p = 1; m() { return this.#p + 'str' } } new A().m()",
                &opts,
                lazy,
                prep,
            );
            check(
                "`a${1}b` + String.raw`x` + [1,2].map(x => x * 2)",
                &opts,
                lazy,
                prep,
            );
        }
    }
}

#[test]
fn a_bad_blob_compiles() {
    let opts = CompileOptions::script("t.js");
    let mut a = vm(false);
    let (_, blob) = a.compile_cached(b"1 + 1", &opts, None).unwrap();
    let mut blob = blob.unwrap();
    let mut b = vm(false);
    // Another text.
    let (f, again) = b.compile_cached(b"2 + 2", &opts, Some(&blob)).unwrap();
    assert!(again.is_some());
    assert_eq!(run(&mut b, f), "4");
    // A truncated blob.
    blob.truncate(blob.len() - 3);
    let mut c = vm(false);
    let (f, again) = c.compile_cached(b"1 + 1", &opts, Some(&blob)).unwrap();
    assert!(again.is_some());
    assert_eq!(run(&mut c, f), "2");
}

#[test]
fn hits_over_the_corpus() {
    extern crate std;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let mut files = Vec::new();
    for area in ["codemode", "codemode-harvest"] {
        for e in std::fs::read_dir(root.join(area)).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "js") {
                files.push(p);
            }
        }
    }
    files.sort();
    let opts = CompileOptions::async_body("codemode.js", &["tools", "console"]);
    let mut n = 0;
    for p in files {
        let src = std::fs::read_to_string(&p).unwrap();
        for lazy in [false, true] {
            n += usize::from(check_compile_only(&src, &opts, lazy));
        }
    }
    assert!(n > 500, "{n} hits");
}

/// [`check`] without running (corpus cases need their host).
fn check_compile_only(src: &str, opts: &CompileOptions<'_>, lazy: bool) -> bool {
    let mut a = vm(lazy);
    let mut b = vm(lazy);
    let Ok((_, Some(blob))) = a.compile_cached(src.as_bytes(), opts, None) else {
        return false;
    };
    let mut c = vm(lazy);
    c.compile(src.as_bytes(), opts).unwrap();
    b.compile_cached(src.as_bytes(), opts, Some(&blob)).unwrap();
    assert!(
        c.heap.arena().bytes() == b.heap.arena().bytes(),
        "heaps differ: {src:.80}"
    );
    assert!(
        a.heap.arena().bytes() == b.heap.arena().bytes(),
        "logging changed the compile: {src:.80}"
    );
    true
}

/// `PROBE=file cargo test --release ... probe_import_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn probe_import_cost() {
    extern crate std;
    use std::time::Instant;
    let src = std::fs::read_to_string(std::env::var("PROBE").unwrap()).unwrap();
    let opts = CompileOptions::async_body("codemode.js", &["tools", "console"]);
    for lazy in [false, true] {
        let mut a = vm(lazy);
        let (_, blob) = a.compile_cached(src.as_bytes(), &opts, None).unwrap();
        let blob = blob.unwrap();
        let (mut tc, mut ti, mut tl) = (u128::MAX, u128::MAX, u128::MAX);
        for _ in 0..50 {
            let mut c = vm(lazy);
            let t = Instant::now();
            c.compile(src.as_bytes(), &opts).unwrap();
            tc = tc.min(t.elapsed().as_nanos());
            let mut l = vm(lazy);
            let t = Instant::now();
            l.compile_cached(src.as_bytes(), &opts, None).unwrap();
            tl = tl.min(t.elapsed().as_nanos());
            let mut b = vm(lazy);
            let t = Instant::now();
            b.compile_cached(src.as_bytes(), &opts, Some(&blob))
                .unwrap();
            ti = ti.min(t.elapsed().as_nanos());
        }
        std::println!(
            "lazy {lazy}: {} B source, blob {} B: compile {} us, compile+export {} us, import {} us",
            src.len(),
            blob.len(),
            tc / 1000,
            tl / 1000,
            ti / 1000
        );
    }
}

/// Suspension maps through bytes are the maps (D3041).
#[cfg(feature = "dump")]
#[test]
fn dump_maps_round_trip() {
    extern crate std;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/codemode-harvest");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "js") {
            continue;
        }
        let src = std::fs::read_to_string(&p).unwrap();
        let opts = CompileOptions::async_body("t.js", &[]);
        let a = vm(false);
        let mut h = crate::heap::Heap::new().unwrap();
        let Ok(maps) = crate::compiler::suspension_maps(&mut h, src.as_bytes(), &opts) else {
            continue;
        };
        let want: Vec<String> = maps.iter().map(|m| m.text(src.as_bytes())).collect();
        {
            let mut c = a.dump_cache.borrow_mut();
            c.keep(opts.bits(), &src, maps, false, Vec::new());
            c.changed = true;
        }
        let bytes = a.export_dump_maps();
        let b = vm(false);
        assert!(b.import_dump_maps(&bytes));
        let c = b.dump_cache.borrow();
        let got: Vec<String> = c.maps[0]
            .3
            .as_ref()
            .unwrap()
            .iter()
            .map(|m| m.text(src.as_bytes()))
            .collect();
        assert_eq!(got, want);
        n += want.len();
    }
    assert!(n > 50, "{n} maps");
}
