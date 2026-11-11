//! TextEncoder and TextDecoder (0244, D3172). qjs has neither, so node is the oracle:
//! tests/data/encoding/cases.expected is node's transcript of cases.js (its
//! gen-expected.sh), and djs must print the same. The deliberate differences from node
//! are below: djs converts the arguments as WebIDL says (in order, all before the steps),
//! knows only UTF-8, and has its own messages where node's name its internals.

use alloc::string::String;

use super::*;

fn vm() -> Vm {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.prepare_base_image().unwrap();
    vm
}

/// The completion value of the strict script `src`, `throw <error>` if it throws.
fn eval(vm: &mut Vm, src: &str) -> String {
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    match vm.run(u64::MAX) {
        Status::Done(v) => vm.display(v),
        Status::Threw(v) => alloc::format!("throw {}", vm.display(v)),
        s => panic!("{s:?}"),
    }
}

#[test]
fn node_transcript() {
    let got = eval(
        &mut vm(),
        include_str!("../../../../tests/data/encoding/cases.js"),
    );
    let want = include_str!("../../../../tests/data/encoding/cases.expected");
    for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
        assert_eq!(g, w, "line {}", i + 1);
    }
    assert_eq!(got.lines().count(), want.lines().count());
}

#[test]
fn messages() {
    let mut vm = vm();
    let cases = [
        (
            "TextEncoder.prototype.encode.call({})",
            "TypeError: Value of \"this\" must be of type TextEncoder",
        ),
        (
            "TextDecoder.prototype.decode.call(new TextEncoder())",
            "TypeError: Value of \"this\" must be of type TextDecoder",
        ),
        ("TextDecoder()", "TypeError: must be called with new"),
        (
            "new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array([0xff]))",
            "TypeError: The encoded data was not valid for encoding utf-8",
        ),
        // node knows these labels; djs only UTF-8's.
        (
            "new TextDecoder('latin1')",
            "RangeError: The \"latin1\" encoding is not supported",
        ),
        (
            "new TextDecoder('utf-16le')",
            "RangeError: The \"utf-16le\" encoding is not supported",
        ),
        (
            "new TextDecoder('utf-8', 1)",
            "TypeError: The \"options\" argument must be of type object",
        ),
        (
            "new TextDecoder().decode([1])",
            "TypeError: The \"input\" argument must be an instance of ArrayBuffer or ArrayBufferView",
        ),
        (
            "new TextEncoder().encodeInto('a', new Uint16Array(1))",
            "TypeError: The \"dest\" argument must be an instance of Uint8Array",
        ),
        (
            "new TextEncoder().encodeInto('a')",
            "TypeError: 2 arguments required",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut vm, src), alloc::format!("throw {want}"), "{src}");
    }
}

/// WebIDL's order, where node's differs: every argument is converted before the steps
/// (so the options are read before an unknown label throws), the argument count is
/// checked before any conversion, and encodeInto's source is a USVString like any other.
#[test]
fn webidl_conversions() {
    let mut vm = vm();
    let cases = [
        (
            "const log = []; try { new TextDecoder({ toString() { log.push('label'); return 'x'; } }, \
             { get fatal() { log.push('fatal'); }, get ignoreBOM() { log.push('ignoreBOM'); } }); } \
             catch (e) { log.push(e.name); } log.join()",
            "label,fatal,ignoreBOM,RangeError",
        ),
        (
            "JSON.stringify(new TextEncoder().encodeInto(123, new Uint8Array(5)))",
            "{\"read\":3,\"written\":3}",
        ),
        (
            "const log = []; try { new TextEncoder().encodeInto({ toString() { log.push('source'); return 'a'; } }, 5); } \
             catch (e) { log.push(e.name); } log.join()",
            "source,TypeError",
        ),
        (
            "const log = []; try { new TextEncoder().encodeInto({ toString() { log.push('source'); return 'a'; } }); } \
             catch (e) { log.push(e.name); } log.join()",
            "TypeError",
        ),
        // The input's type is checked before the options are read.
        (
            "const log = []; try { new TextDecoder().decode(5, { get stream() { log.push('stream'); } }); } \
             catch (e) { log.push(e.name); } log.join()",
            "TypeError",
        ),
        // A Proxy new.target: its `prototype` is read after the arguments are converted.
        (
            "const log = []; const nt = new Proxy(function () {}, { get(t, k) { log.push(String(k)); return t[k]; } }); \
             Reflect.construct(TextDecoder, [{ toString() { log.push('label'); return 'utf8'; } }], nt); log.join()",
            "label,prototype",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut vm, &alloc::format!("{{ {src} }}")), want, "{src}");
    }
}

/// Decoding and encoding are charged by the byte.
#[test]
fn fuel_by_bytes() {
    let mut vm = vm();
    let fuel = |vm: &mut Vm, src: &str| {
        let f = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::script("t.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
        let (status, used) = vm.run_metered(u64::MAX);
        assert!(matches!(status, Status::Done(_)));
        used
    };
    eval(
        &mut vm,
        "var big = new Uint8Array(1 << 20).fill(0x61); var s = 'a'.repeat(1 << 20);",
    );
    let small = fuel(&mut vm, "new TextDecoder().decode(big.subarray(0, 8))");
    let large = fuel(&mut vm, "new TextDecoder().decode(big)");
    assert!(small < 1 << 12 && large >= 1 << 17, "{small} {large}");
    let small = fuel(&mut vm, "new TextEncoder().encode(s.slice(0, 8))");
    let large = fuel(&mut vm, "new TextEncoder().encode(s)");
    assert!(small < 1 << 12 && large >= 1 << 17, "{small} {large}");
}
