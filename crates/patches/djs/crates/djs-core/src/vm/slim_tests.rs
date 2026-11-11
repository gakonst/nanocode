//! The slim build (no `full` feature, D2411): the families it leaves out are gone with a
//! message that says so, everything else is there. The same tests run in the full build
//! and check the opposite, so both builds keep agreeing on the boundary.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::FULL;

/// A VM like the embedders' (the realm and prelude, prepared as a base image).
fn vm() -> Vm {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.prepare_base_image().unwrap();
    vm
}

/// The result of `src` as a strict script: its completion value, `throw <message>` for a
/// runtime error, `syntax <message>` for one at compile time.
fn eval(vm: &mut Vm, src: &str) -> String {
    let opts = crate::compiler::CompileOptions::script("t.js");
    let f = match vm.compile(src.as_bytes(), &opts) {
        Ok(f) => f,
        Err(e) => return alloc::format!("syntax {}", e.message),
    };
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    match vm.run(u64::MAX) {
        Status::Done(v) => vm.display(v),
        Status::Threw(v) => alloc::format!("throw {}", vm.display(v)),
        s => panic!("{s:?}"),
    }
}

/// The globals the slim build leaves out, and the embed-only ones: `typeof` is
/// "undefined" and a use names the build; in builds that have them they are functions.
#[test]
fn absent_globals() {
    let mut vm = vm();
    let realm = crate::interp::realm::ABSENT_GLOBALS
        .iter()
        .map(|n| (n, FULL));
    let embed = crate::interp::realm::EMBED_GLOBALS
        .iter()
        .map(|n| (n, crate::EMBED));
    for (name, present) in realm.chain(embed) {
        let t = eval(&mut vm, &alloc::format!("typeof {name}"));
        let used = eval(&mut vm, &alloc::format!("'use strict'; {name}; 1"));
        if present {
            assert_eq!(t, "function", "{name}");
            assert_eq!(used, "1", "{name}");
        } else {
            assert_eq!(t, "undefined", "{name}");
            assert_eq!(
                used,
                alloc::format!("throw ReferenceError: '{name}' is not available in this build"),
            );
        }
    }
    // Other missing names keep the usual message.
    let r = eval(&mut vm, "nope");
    assert_eq!(r, "throw ReferenceError: 'nope' is not defined");
}

/// The syntax of the families the slim build leaves out is a SyntaxError that names it.
#[test]
fn absent_syntax() {
    let mut vm = vm();
    let cases = [
        (
            "1n + 2n",
            "3n",
            "BigInt literals are not available in this build",
        ),
        (
            "function d(c) { return c } @d class A {} typeof A",
            "function",
            "decorators are not available in this build",
        ),
        (
            "{ using x = null; } 1",
            "1",
            "`using` declarations are not available in this build",
        ),
    ];
    for (src, full, slim) in cases {
        let got = eval(&mut vm, src);
        if FULL {
            assert_eq!(got, full, "{src}");
        } else {
            assert!(
                got.starts_with("syntax ") && got.contains(slim),
                "{src}: {got}"
            );
        }
    }
}

/// Members of kept objects that come from the left-out families: Iterator helpers on
/// %IteratorPrototype%, `Symbol.dispose`, `%TypedArray%.prototype.toString`.
#[test]
fn absent_members() {
    let mut vm = vm();
    let got: Vec<String> = [
        "typeof [].values().map",
        "typeof [].values().toArray",
        "typeof Symbol.dispose",
        "typeof Symbol.asyncDispose",
        "typeof Object.getPrototypeOf([].values())[Symbol.dispose]",
    ]
    .iter()
    .map(|s| eval(&mut vm, s))
    .collect();
    let want = if FULL {
        ["function", "function", "symbol", "symbol", "function"]
    } else {
        ["undefined"; 5]
    };
    assert_eq!(got, want);
}

/// What codemode scripts use is all there in both builds: the protocols behind for-of,
/// spread and `await`, JSON, Proxy and Reflect (pi's `tools`), Number conversion through
/// ToNumeric, defineProperty and Reflect.set (whose typed-array branches the slim build
/// can't reach), the generator machinery and the frozen-builtins override fix.
#[test]
fn kept_core() {
    let mut vm = vm();
    let cases = [
        ("[...new Set([1, 2, 2])].join()", "1,2"),
        ("Array.from(new Map([[1, 2]]).entries()).join()", "1,2"),
        (
            "JSON.stringify(Object.fromEntries([['a', [1, 'x']]]))",
            "{\"a\":[1,\"x\"]}",
        ),
        (
            "Number({ valueOf() { return '5' } }) + +{ valueOf() { return 2 } }",
            "7",
        ),
        (
            "const o1 = {}; Object.defineProperty(o1, 'a', { value: 1 }); o1.a",
            "1",
        ),
        ("const o2 = {}; Reflect.set(o2, 'b', 2) && o2.b", "2"),
        (
            "new Proxy({}, { get: (t, k) => Reflect.get({ x: 3 }, k) }).x",
            "3",
        ),
        ("function* g() { yield 1; yield 2 } [...g()].join()", "1,2"),
        ("let n = 0; for (const c of 'ab') n++; n", "2"),
        ("[3, 1, 2].sort().map(x => x * 2).join()", "2,4,6"),
        (
            "typeof WeakMap + typeof Symbol + typeof Promise.allSettled",
            "functionfunctionfunction",
        ),
        ("let x1 = { valueOf() { return 1 } }; x1++; x1", "2"),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut vm, src), want, "{src}");
    }
}
