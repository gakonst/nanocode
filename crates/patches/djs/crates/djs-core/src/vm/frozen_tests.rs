//! The frozen-builtins mode (D022, D2304): builtins behave as frozen, with the override
//! fix, on a VM made fresh and on one made from a base image; the default is unchanged.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;

fn frozen_opts() -> VmOptions {
    VmOptions {
        frozen_builtins: true,
        ..VmOptions::default()
    }
}

/// Runs `src` (a script whose completion value is the result) to its end, in slices of
/// `SLICE` fuel, restoring the image at every stop.
fn eval(vm: &mut Vm, src: &str) -> String {
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let slice = SLICE.with(|s| s.get());
    loop {
        match vm.run(slice) {
            Status::Done(v) => return vm.display(v),
            Status::Threw(v) => return alloc::format!("throw {}", vm.display(v)),
            Status::OutOfFuel => {
                let opts = vm.options();
                *vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
            }
            s => panic!("{s:?}"),
        }
    }
}

extern crate std;
std::thread_local! {
    /// The fuel slice of `eval` (u64::MAX: one run).
    static SLICE: core::cell::Cell<u64> = const { core::cell::Cell::new(u64::MAX) };
}

/// Each line of `cases` is `expression => expected`; the expression runs in a strict
/// script of its own (a `try` around it turns a throw into the error's constructor name).
fn check(vm: &mut Vm, cases: &str) {
    let mut failed = Vec::new();
    for line in cases.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (expr, want) = line.rsplit_once(" => ").expect("case");
        let src = alloc::format!(
            "'use strict'; (() => {{ try {{ return String({expr}); }} \
             catch (e) {{ return 'throws ' + e.constructor.name; }} }})()"
        );
        let got = eval(vm, &src);
        if got != want {
            failed.push(alloc::format!("{expr}: got {got}, want {want}"));
        }
    }
    assert!(failed.is_empty(), "\n{}", failed.join("\n"));
}

const FROZEN: &str = r#"
    Object.isFrozen(Array.prototype) => true
    Object.isFrozen(Object.prototype) => true
    Object.isFrozen(Math) => true
    Object.isFrozen(Array) => true
    Object.isFrozen(Array.prototype.push) => true
    Object.isSealed(JSON) => true
    Object.isExtensible(Function.prototype) => false
    Reflect.isExtensible(Error.prototype) => false
    Object.isFrozen(globalThis) => false
    Object.isExtensible(globalThis) => true
    JSON.stringify(Object.getOwnPropertyDescriptor(Array.prototype, 'push')).replace(/"value":[^,]*,/, '') => {"writable":false,"enumerable":false,"configurable":false}
    JSON.stringify(Object.getOwnPropertyDescriptor(Math, 'PI')) => {"value":3.141592653589793,"writable":false,"enumerable":false,"configurable":false}
    Object.getOwnPropertyDescriptor(Map.prototype, 'size').configurable => false
    typeof Object.getOwnPropertyDescriptor(Map.prototype, 'size').get => function
    Array.prototype.push = 1 => throws TypeError
    Math.PI = 3 => throws TypeError
    Array.prototype.extra = 1 => throws TypeError
    Array.prototype[0] = 1 => throws TypeError
    Array.prototype.length = 3 => throws TypeError
    Object.prototype.x = 1 => throws TypeError
    delete Array.prototype.push => throws TypeError
    delete Math.nothing => true
    Reflect.deleteProperty(Math, 'PI') => false
    Reflect.set(Math, 'PI', 3) => false
    Reflect.set(Math, 'E', Math.E) => false
    Reflect.defineProperty(Math, 'PI', { value: 3 }) => false
    Reflect.defineProperty(Math, 'PI', { value: Math.PI }) => true
    Reflect.defineProperty(Math, 'PI', { writable: false, configurable: false }) => true
    Reflect.defineProperty(Math, 'PI', { enumerable: true }) => false
    Reflect.defineProperty(Math, 'TAU', { value: 6 }) => false
    Object.defineProperty(Array.prototype, 'x', { value: 1 }) => throws TypeError
    Reflect.setPrototypeOf(Array.prototype, null) => false
    Reflect.setPrototypeOf(Array.prototype, Object.prototype) => true
    Object.setPrototypeOf(Math, {}) => throws TypeError
    Reflect.preventExtensions(Math) => true
    Object.freeze(Array.prototype) === Array.prototype => true
    Object.seal(Math) === Math => true
    Array.prototype.push.call(Array.prototype, 1) => throws TypeError
    Array.prototype.length => 0
    Object.keys(Math).length => 0
    Math.PI => 3.141592653589793
    (() => { const e = new Error('m'); e.name = 'X'; return e.name; })() => X
    (() => { class E extends Error { constructor(m) { super(m); this.name = 'E'; } } return String(new E('y')); })() => E: y
    (() => { const o = {}; o.toString = () => 'own'; return String(o) + Object.keys(o); })() => owntoString
    (() => { const a = []; a.push = 1; return typeof a.push + typeof Array.prototype.push; })() => numberfunction
    (() => { const o = Object.create(Math); o.PI = 3; return o.PI + Math.PI; })() => 6.141592653589793
    (() => { const f = function () {}; f.prototype.constructor = 1; return f.prototype.constructor; })() => 1
    (() => { const o = {}; Reflect.set(Object.prototype, 'valueOf', 5, o); return o.valueOf; })() => 5
    (() => { class A extends Error { m() { super.name = 'via super'; return this.name; } } return new A().m(); })() => via super
    (() => { class A { m() { super.toString = 1; return this.toString; } } return new A().m(); })() => 1
    (() => { const o = { m() { super.valueOf = 2; return Object.keys(this).join(); } }; return o.m(); })() => m,valueOf
    (() => { const o = Object.create(Array.prototype); o.length = 3; o[0] = 1; return o.length + Object.keys(o).join(); })() => 30,length
    (() => { const s = Object.create(String.prototype); s.length = 2; return s.length; })() => 2
    (() => { 'use strict'; const n = 1; n.toString = 1; })() => throws TypeError
    Reflect.set(Object.prototype, 'valueOf', 5, Array.prototype) => false
    (() => { const o = Object.freeze(Object.create(Object.prototype)); return Reflect.set(o, 'toString', 1); })() => false
    (() => { const o = Object.create(Object.prototype, { toString: { get() { return 1; } } }); return Reflect.set(o, 'toString', 2); })() => false
    (() => { const p = new Proxy(Array.prototype, {}); p.extra = 1; })() => throws TypeError
    (() => { const p = new Proxy(Math, {}); return Reflect.set(p, 'PI', 1); })() => false
    (() => { const p = new Proxy(Math, {}); return Object.isFrozen(p); })() => true
    (() => { const p = new Proxy({}, {}); const o = Object.create(p); o.x = 1; return o.x; })() => 1
    (() => { const o = Object.create(new Proxy(Object.prototype, {})); o.toString = 1; return o.toString; })() => 1
    (() => { globalThis.mine = 1; mine = 2; return mine; })() => 2
    (() => { globalThis.Array2 = Array; delete globalThis.Array2; return typeof Array2; })() => undefined
    (() => { class A { #x = 1; static has(o) { return #x in o; } } class B extends class { constructor(o) { return o; } } { #y = 1; } new B(Math); })() => throws TypeError
    (() => { const a = [1, 2, 3]; a.length = 1; a.push(4); return a.join(); })() => 1,4
    (() => { const m = new Map([[1, 2]]); m.set(3, 4); return m.size; })() => 2
    Symbol.for('frozen-test') === Symbol.for('frozen-test') => true
    [3, 1, 2].sort().join() => 1,2,3
    JSON.stringify({ a: [1, { b: 2 }] }) => {"a":[1,{"b":2}]}
    Array.prototype.constructor = Object => throws TypeError
    Reflect.defineProperty(Array, Symbol.species, { get() { return Object; } }) => false
    [1, 2].map((x) => x * 2).constructor === Array => true
    (() => { class S extends Array {} return S.from([1]).map((x) => x) instanceof S; })() => true
    (() => { const a = [1]; a.constructor = { [Symbol.species]: function (n) { this.n = n; } }; return a.map((x) => x).n; })() => 1
    (() => { const a = [1]; Object.setPrototypeOf(a, Object.create(Array.prototype, { constructor: { value: undefined } })); return Array.isArray(a.filter(() => true)); })() => true
"#;

#[test]
fn frozen_builtins_on_a_fresh_vm() {
    let mut vm = Vm::new(frozen_opts()).unwrap();
    assert!(vm.frozen_builtins());
    check(&mut vm, FROZEN);
}

#[test]
fn frozen_builtins_under_gc_stress_and_suspension() {
    let mut opts = frozen_opts();
    opts.exec.gc_stress = true;
    let mut base = Vm::new(VmOptions::default()).unwrap();
    base.prepare_base_image().unwrap();
    let mut vm = Vm::from_base(base.bytes(), opts).unwrap();
    SLICE.with(|s| s.set(97));
    check(&mut vm, FROZEN);
    SLICE.with(|s| s.set(u64::MAX));
}

#[test]
fn frozen_builtins_on_a_base_image() {
    let mut base = Vm::new(VmOptions::default()).unwrap();
    base.prepare_base_image().unwrap();
    let mut vm = Vm::from_base(base.bytes(), frozen_opts()).unwrap();
    check(&mut vm, FROZEN);
    // The mode is part of the image; collections keep the boundary.
    vm.collect().unwrap();
    vm.collect_canonical().unwrap();
    let mut vm = Vm::from_bytes(vm.image().to_vec(), VmOptions::default()).unwrap();
    assert!(vm.frozen_builtins());
    check(&mut vm, FROZEN);
    // The same base without the option is ordinary.
    let mut plain = Vm::from_base(base.bytes(), VmOptions::default()).unwrap();
    assert!(!plain.frozen_builtins());
    check(
        &mut plain,
        r#"
        Object.isFrozen(Array.prototype) => false
        (() => { Array.prototype.extra = 1; return [].extra; })() => 1
        (() => { const o = Object.create(Object.freeze({ x: 1 })); o.x = 2; })() => throws TypeError
        "#,
    );
}

#[test]
fn frozen_builtins_keep_non_builtins_ordinary() {
    let mut vm = Vm::new(frozen_opts()).unwrap();
    // A frozen ordinary object still has the spec's override mistake.
    check(
        &mut vm,
        r#"
        (() => { const p = Object.freeze({ x: 1 }); const o = Object.create(p); o.x = 2; })() => throws TypeError
        (() => { const p = Object.freeze({ x: 1 }); return Reflect.set(Object.create(p), 'x', 2); })() => false
        (() => { const o = {}; Object.defineProperty(o, 'x', { value: 1 }); o.x = 2; })() => throws TypeError
        (() => { const o = Object.freeze([1]); o.push(2); })() => throws TypeError
        "#,
    );
}
