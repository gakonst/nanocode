//! TypeScript tests (compiler/ts.rs): erasure end to end, syntax errors, strip-types.
//! Outputs and errors were checked against dquickts (`qjs` on a `.ts` file).

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

use super::tests::{run_body_opts, run_opts};
use super::*;
use crate::heap::Heap;

fn run_ts_cases(cases: &[(&str, &str)]) {
    let mut bad = Vec::new();
    for &(src, want) in cases {
        let got = run_body_opts(src, true);
        if got != want {
            bad.push(alloc::format!("{src}\n  got:  {got:?}\n  want: {want:?}"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

#[test]
fn ts_annotations() {
    run_ts_cases(&[
        ("let a: number = 1; return a", "=> 1"),
        ("let b!: string; return typeof b", "=> undefined"),
        (
            "const c: readonly number[] = [1, 2]; let d: [x: number, y?: string, ...r: boolean[]] = [1]; return c.length + d.length",
            "=> 3",
        ),
        (
            "function add(x: number, y?: number, ...rest: number[]): number { return x + (y ?? 0) + rest.length } return add(1, 2, 3, 4)",
            "=> 5",
        ),
        (
            "function f(this: { k: number }, n: number): number { return this.k + n } return f.call({ k: 1 }, 2) + ':' + f.length",
            "=> 3:1",
        ),
        (
            "function d({ p, q }: { p: number; q?: number } = { p: 1 }, [r]: [number] = [2]): number { return p + (q ?? 0) + r } return d() + d({ p: 1, q: 2 }, [3])",
            "=> 9",
        ),
        (
            "let { a, b }: { a: number, b: number } = { a: 1, b: 2 }; return a + b",
            "=> 3",
        ),
        ("try { throw 1 } catch (e: unknown) { return e }", "=> 1"),
        (
            "let s = 0; for (let i: number = 0; i < 3; i++) s += i; return s",
            "=> 3",
        ),
        (
            "function isS(x: unknown): x is string { return typeof x === 'string' } function an(x: unknown): asserts x is number {} an(1); return isS('s')",
            "=> true",
        ),
        (
            "let fn: (x: number, ...r: string[]) => void = () => {}; let ctor: abstract new (...a: unknown[]) => object; let sym: unique symbol; return typeof fn + typeof ctor + typeof sym",
            "=> functionundefinedundefined",
        ),
        // A line break ends a type: `<number>(5)` is an expression statement.
        (
            "let r: number\n<number>(5); return typeof r",
            "=> undefined",
        ),
    ]);
}

#[test]
fn ts_declarations() {
    run_ts_cases(&[
        (
            "type A<in out T, const U = T> = T | U; interface I<T> extends Object { a?: T; readonly b: number; [k: string]: unknown; m?(): void; new (x: number): I<T>; <U>(u: U): U } return 1",
            "=> 1",
        ),
        (
            "type Obj = { a: number; -readonly [K in 'd']-?: number }; type M<T> = { +readonly [K in keyof T as `get${Capitalize<string & K>}`]+?: () => T[K] }; type C<T> = T extends (infer U extends string)[] ? U : T extends Promise<infer V> ? V : never; return 2",
            "=> 2",
        ),
        (
            "type Q = typeof globalThis.Math.PI | keyof typeof Math; type Imp = typeof import('./x.js') | import('./x.js').Foo<number>; type N = Array<Array<Map<string, Set<number>>>>; type L = | 'a' | 'b'; type T = `a${number}b`; return 3",
            "=> 3",
        ),
        (
            "declare var v: number; declare let l: string, l2: number; declare const c: number; declare function f(x: number): void; declare class K<T> extends Object implements I<T> { m(): void } declare enum E { A } declare const enum F { A } declare namespace N { const x: number } declare module M {} declare module 'm' {} declare global { interface G {} } return typeof v + typeof K + typeof E + typeof N",
            "=> undefinedundefinedundefinedundefined",
        ),
        (
            "function o(x: string): string; function o(x: number): number; function o(x: any) { return x } return o(4)",
            "=> 4",
        ),
        // Contextual keywords stay identifiers; a line break ends the statement.
        (
            "var type = 1, declare = 2, abstract = 3, namespace = 4, module = 5, readonly = 6, is = 7, asserts = 8, satisfies = 9, keyof = 10, infer = 11, global = 12; return type + declare + abstract + namespace + module + readonly + is + asserts + satisfies + keyof + infer + global",
            "=> 78",
        ),
        ("var Foo, type; type\nFoo = 1; return Foo", "=> 1"),
        (
            "type: for (;;) { break type } declare: { } return 0",
            "=> 0",
        ),
    ]);
}

#[test]
fn ts_expressions() {
    run_ts_cases(&[
        (
            "function id<T>(x: T): T { return x } const a = 1, b = 2, c = 3; return [id<number>(4), a < b > c, a < b >= c, a < (b > c)].join()",
            "=> 4,false,false,false",
        ),
        (
            "function id<T>(x: T): T { return x } const i = id<string>; return i('s') + new Map<string, number>([['k', 1]]).get('k') + new Array<number>().length",
            "=> s10",
        ),
        (
            "const tag = (s: TemplateStringsArray, ...v: number[]) => s.raw[0] + v.length; return tag<number>`x${1}`",
            "=> x1",
        ),
        (
            "const a = 1, b = 2; return [(1 + 2 as number) * 2, a as number < b, b as number * 3 + 1, b as number ** 3 ** 2, 1 + b satisfies number ** 2 - 1].join()",
            "=> 6,true,7,512,8",
        ),
        (
            "const o: { p?: { q: number } } = { p: { q: 5 } }; let v = <number><unknown>'abc'; return o.p!.q + o.p!['q']! + v",
            "=> 10abc",
        ),
        (
            "let x = 1; (x as number) = 2; (<number>x)++; x! += 1; return x",
            "=> 4",
        ),
        (
            "const u = { k: 1 } as unknown as { k: number }; const arr = [1, 2] as const; const cfg = { port: 80 } satisfies { port: number }; return u.k + arr.length + cfg.port",
            "=> 83",
        ),
        ("let k = 1; let t = k as\n number; return t", "=> 1"),
    ]);
}

#[test]
fn ts_arrows() {
    run_ts_cases(&[
        (
            "const f1 = (n: number): number => n + 1; const f3 = (n: number): { r: number } => ({ r: n }); const f4 = (): (() => number) => () => 7; return f1(1) + f3(2).r + f4()()",
            "=> 11",
        ),
        (
            "const g = <T extends object = {}>(x: T): T => x; const cg = <const T extends readonly unknown[]>(x: T): T => x; const ag = async <T,>(x: T): Promise<T> => x; return g({ v: 2 }).v + cg([1, 2] as const).length + typeof ag(1).then",
            "=> 4function",
        ),
        (
            "const arrow = (x: number, y: string = 'y'): string => x + y; const f2 = async (n: number): Promise<number> => n; return arrow(1) + typeof f2",
            "=> 1yfunction",
        ),
        // `x ? (y) : z => w` is a conditional; the arrow takes the return type only
        // when another `:` follows its body.
        (
            "const t = true; let y = 10, z = 20; const c1 = t ? (y) : z => z; const c2 = !t ? (y) : (z) => z; const c3 = t ? (n: number): number => n * 2 : null; const c4 = t ? (n): string => 'r' + n : (n) => n; return c1 + typeof c2 + c3!(4) + c4(1)",
            "=> 10function8r1",
        ),
        (
            "const obj = { m<T>(x: T): T { return x }, get g(): number { return 1 }, set s(v: number) {}, async am<T>(): Promise<void> {} }; return obj.m(2) + obj.g",
            "=> 3",
        ),
        (
            "const gen = function* <T>(x: T): Generator<T> { yield x }; return [...gen(5)][0]",
            "=> 5",
        ),
    ]);
}

#[test]
fn ts_classes() {
    run_ts_cases(&[
        (
            "abstract class Shape { abstract area(): number; abstract readonly kind: string; toString(): string { return this.kind + this.area() } } class Sq extends Shape implements Iterable<number> { readonly kind = 'sq'; [key: string]: unknown; declare extra: number; static readonly count: number = 0; private static secret?: string; protected optional?: number; definite!: number; side = 3; area(): number { return this.side ** 2 } public override toString(): string { return '[' + super.toString() + ']' } *[Symbol.iterator](): Iterator<number> { yield this.side } method?(): void; overload(x: string): string; overload(x: number): number; overload(x: unknown): unknown { return x } } const s = new Sq(); return String(s) + [...s][0] + s.overload(1) + ('extra' in s) + typeof s.method + Object.keys(s).join()",
            "=> [sq9]31falseundefinedkind,optional,definite,side",
        ),
        (
            "class M { readonly = 1; declare() { return 2 } abstract?: number; static public = 3; get<T>(x: T) { return x } set?: number; is = 4 } const m = new M(); return m.readonly + m.declare() + M.public + m.get(4) + m.is",
            "=> 14",
        ),
        (
            "const Box = class<T> { v: T; constructor(v: T) { this.v = v } }; class Pair<in out K, const V = unknown> extends Map<K, V> {} return new Box<number>(2).v + new Pair<string, number>().size",
            "=> 2",
        ),
    ]);
}

#[test]
fn ts_enums() {
    run_ts_cases(&[
        (
            "enum E { A, B, C = 10, D, E = 'e', F = D + 1 } return JSON.stringify(E) + E[11] + E['e']",
            "=> {\"0\":\"A\",\"1\":\"B\",\"10\":\"C\",\"11\":\"D\",\"12\":\"F\",\"A\":0,\"B\":1,\"C\":10,\"D\":11,\"E\":\"e\",\"F\":12}Dundefined",
        ),
        (
            "let n = 0; function nx() { return ++n * 100 } enum C { X = nx(), Y = X + 1, Z = 'abc'.length, W = Z * 2 } return [C.X, C.Y, C[101], C.W].join()",
            "=> 100,101,Y,6",
        ),
        (
            "enum M { A = 1 } enum M { B = A + 1, C = B * 2 } enum N { 'a-b' = 1, if = 2, default = N.if + 1 } return M.A + M.B + M.C + N['a-b'] + N.if + N.default",
            "=> 13",
        ),
        (
            "enum K { A = 1 << 4, B = A | 3, C = ~B, D = -(2 ** 3), E = 'x' + 'y', F = 7 % 4, G = 100 >>> 2, H = (1 + 2) * 3 } return [K.A, K.B, K.C, K.D, K.E, K.F, K.G, K.H].join()",
            "=> 16,19,-20,-8,xy,3,25,9",
        ),
        (
            "enum Color { Red, Green } namespace Color { export function name(c: Color) { return Color[c] } } { enum Inner { A = 5 } if (Inner.A !== 5) return 'no' } return Color.name(Color.Green) + typeof Inner",
            "=> Greenundefined",
        ),
        // A const enum is folded where it is read (as tsc): redefining a member does
        // not change the compiled code, computed keys use the object.
        (
            "function early() { return Later.A } const enum CE { A = 1, B = A * 10, S = 'str' } const enum Later { A = 5 } function use() { return CE.B + CE['A'] } Object.defineProperty(CE, 'B', { get() { return 99 } }); Object.defineProperty(Later, 'A', { get() { return 99 } }); const key = 'B'; return [use(), early(), CE[key], CE.S, Object.keys(Later).join('/')].join()",
            "=> 11,5,99,str,5/A",
        ),
        (
            "namespace NS { export const enum K { X = 7 } export const via = K.X } function useNS() { return NS.K.X } Object.defineProperty(NS.K, 'X', { get() { return 0 } }); function shadow(CE: { B: number }) { return CE.B } const enum CE { B = 2 } return useNS() + NS.via + shadow({ B: 3 })",
            "=> 17",
        ),
    ]);
}

#[test]
fn ts_namespaces() {
    run_ts_cases(&[
        (
            "namespace A.B.C { export const v = 1 } return A.B.C.v",
            "=> 1",
        ),
        (
            "namespace Counter { export let count = 0; export function inc() { return ++count } let hidden = 5; export const [first, second] = [hidden, hidden + 1]; export const { prop } = { prop: 'p' } } Counter.inc(); Counter.count += 10; return [Counter.count, Counter.first + Counter.second, Counter.prop, Object.keys(Counter).includes('hidden')].join()",
            "=> 11,11,p,false",
        ),
        (
            "namespace Merged { export const x = 1 } namespace Merged { export const y = x + 1; export function f() { return z } } namespace Merged { export const z = y + 1 } return Merged.f()",
            "=> 3",
        ),
        (
            "function Fn() { return 1 } namespace Fn { export const extra = 2 } class Cls { static s = 1 } namespace Cls { export const t = 2 } return Fn() + Fn.extra + Cls.s + Cls.t",
            "=> 6",
        ),
        (
            "namespace Outer { const secret = 10; export namespace Inner { export const w = secret + 1 } export class K { v = Inner.w } export enum E { A = Inner.w } export import Alias = Inner.w } return Outer.Inner.w + new Outer.K().v + Outer.E.A + Outer.Alias + Outer.E[11]",
            "=> 44A",
        ),
        (
            "namespace Types { export interface I {} export type T = number; namespace Deep { export type U = string } } namespace Ambient { export declare let late: number; export function read() { return late } } (Ambient as any).late = 8; import Alias = Ambient; import Gone = Types.T; return typeof Types + Alias.read() + typeof Gone",
            "=> undefined8undefined",
        ),
    ]);
}

/// Parameter properties: defined before the other fields, assigned after the
/// parameters (after `super()` in a derived class), as tsc with
/// `useDefineForClassFields`.
#[test]
fn ts_parameter_properties() {
    run_ts_cases(&[
        (
            "const log: string[] = []; class Base { constructor(public b: number) { log.push('base:' + Object.keys(this).join()) } } class Derived extends Base { field = (log.push('field:' + Object.keys(this).join() + ':' + this.x), 1); constructor(public x: number, protected readonly y: string = 'dy', private z?: number) { log.push('ctor'); super(x * 2); log.push('after:' + Object.keys(this).join() + ':' + this.x) } } const d = new Derived(5); return log.join('|') + ' ' + JSON.stringify(d)",
            "=> ctor|base:b|field:b,x,y,z:undefined|after:b,x,y,z,field:5 {\"b\":10,\"x\":5,\"y\":\"dy\",\"field\":1}",
        ),
        (
            "class P { a = this.x; constructor(public x: number = 1, readonly r = this.a) {} } const p = new P(); return [p.a, p.x, Object.keys(p).join()].join()",
            "=> ,1,x,r,a",
        ),
        (
            "class Base { constructor(public b: number) {} } class Q extends Base { constructor(public q: number) { const init = () => super(q); init() } } return new Q(4).q + new Q(4).b",
            "=> 8",
        ),
    ]);
}

fn ts_err(src: &str) -> CompileError {
    let mut heap = Heap::new().unwrap();
    match compile(
        &mut heap,
        src.as_bytes(),
        &CompileOptions::script("t.ts").ts(),
    ) {
        Ok(_) => panic!("{src:?} compiled"),
        Err(e) => e,
    }
}

/// `(source, message, line, column)`, as dquickts reports them.
#[test]
fn ts_errors() {
    let cases: &[(&str, &str, u32, u32)] = &[
        ("let x: = 1;", "unexpected token in type: '='", 1, 8),
        ("function f(x: number: string) {}", "expecting ','", 1, 21),
        (
            "let x: number[ = 1;",
            "unexpected token in type: '='",
            1,
            16,
        ),
        ("type T = ;", "unexpected token in type: ';'", 1, 10),
        ("interface I { a: }", "unexpected token in type: '}'", 1, 18),
        (
            "declare function g() {}",
            "an ambient function cannot have a body",
            1,
            9,
        ),
        (
            "let q: (a: number => void;",
            "unexpected token in type: ':'",
            1,
            10,
        ),
        (
            "type A = { a: number b: string }",
            "unexpected token in type: 'b'",
            1,
            22,
        ),
        ("type A = -x;", "unexpected token in type: 'x'", 1, 11),
        (
            "class C<T extends> {}",
            "unexpected token in type: '>'",
            1,
            18,
        ),
        (
            "let x = <number>;",
            "unexpected token in expression: ';'",
            1,
            17,
        ),
        ("let t: (number", "unexpected end of input in type", 1, 15),
        ("enum E { 1 = 2 }", "invalid enum member name", 1, 10),
        (
            "enum E { A = f(), B }",
            "enum member must have initializer",
            1,
            21,
        ),
        (
            "let x = 1; const enum Bad { A = x }",
            "const enum member initializers must be constant expressions",
            1,
            33,
        ),
        (
            "namespace N { return 1 }",
            "return not in a function",
            1,
            15,
        ),
        (
            "namespace N { export foo }",
            "unsupported export in a namespace",
            1,
            22,
        ),
        (
            "import x = require('x');",
            "'import ... = require()' is not supported (no CommonJS modules), use 'import ... from'",
            1,
            12,
        ),
        ("function f(private) {}", "missing formal parameter", 1, 12),
    ];
    let mut bad = Vec::new();
    for &(src, msg, line, col) in cases {
        let e = ts_err(src);
        if (e.message.as_str(), e.line, e.col) != (msg, line, col) {
            bad.push(alloc::format!(
                "{src:?}: got {:?} {}:{}, want {msg:?} {line}:{col}",
                e.message,
                e.line,
                e.col
            ));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

fn legacy_opts(metadata: bool) -> CompileOptions<'static> {
    CompileOptions {
        experimental_decorators: true,
        emit_decorator_metadata: metadata,
        ..CompileOptions::async_body("t.ts", &["console"]).ts()
    }
}

/// TypeScript legacy decorators (0065 part 2): the parts of dquickts'
/// tests/test_decorators_legacy.ts that the difftest case cannot run, checked with
/// qjs `--experimental-decorators`.
#[test]
fn legacy_decorators() {
    let cases: &[(&str, bool, &str)] = &[
        // Without emitDecoratorMetadata: no metadata decorators.
        (
            "const log = []; const d = (t, k) => { log.push(k); }; Reflect.metadata = () => { log.push('meta'); return d; }; class C { @d x: string; @d m(a: number) {} } return log.join()",
            false,
            "=> x,m",
        ),
        // Reflect.metadata is only used if it exists.
        (
            "const d = (t, k) => {}; class Plain { @d x: string; } return Object.getOwnPropertyNames(Plain.prototype).join()",
            true,
            "=> constructor",
        ),
        (
            "const log = []; Reflect.metadata = (k, v) => (t, key) => { log.push(key + ' ' + k + ' ' + v.name); }; const d = () => {}; class C { @d x: string; } return log.join()",
            true,
            "=> x design:type String",
        ),
        // Parameter decorators of a private method get its name as a string key.
        (
            "let r; const d = (t, k, i) => { r = typeof k + ' ' + k + ' ' + i; }; class C { #m(a, @d b) {} } return r",
            false,
            "=> string #m 1",
        ),
    ];
    let mut bad = Vec::new();
    for &(src, metadata, want) in cases {
        let got = run_opts(src, &legacy_opts(metadata));
        if got != want {
            bad.push(alloc::format!("{src}\n  got:  {got:?}\n  want: {want:?}"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// `(source, legacy decorators, message, line, column)`, as qjs reports them (with
/// `--experimental-decorators` for the legacy ones).
#[test]
fn legacy_decorator_errors() {
    let cases: &[(&str, bool, &str, u32, u32)] = &[
        (
            "class C { @dec #m() {} }",
            true,
            "decorators are not valid here",
            1,
            18,
        ),
        (
            "class C { @dec static #x = 1; }",
            true,
            "decorators are not valid here",
            1,
            26,
        ),
        (
            "class C { @dec accessor #x = 1; }",
            true,
            "decorators are not valid here",
            1,
            28,
        ),
        (
            "class C { @dec constructor() {} }",
            true,
            "decorators are not valid here",
            1,
            27,
        ),
        (
            "class C { @dec static {} }",
            true,
            "decorators are not valid here",
            1,
            23,
        ),
        (
            "@d function f() {}",
            true,
            "decorators must be followed by a class",
            1,
            4,
        ),
        (
            "class C { m(x, @d) {} }",
            true,
            "missing formal parameter",
            1,
            18,
        ),
        (
            "function f(@d x) {}",
            true,
            "parameter decorators are only supported with experimentalDecorators",
            1,
            12,
        ),
        (
            "const o = { m(@d x) {} }",
            true,
            "parameter decorators are only supported with experimentalDecorators",
            1,
            15,
        ),
        (
            "class C { m(@d x) {} }",
            false,
            "parameter decorators are only supported with experimentalDecorators",
            1,
            13,
        ),
        (
            "class C { constructor(@d x) {} }",
            false,
            "parameter decorators are only supported with experimentalDecorators",
            1,
            23,
        ),
    ];
    let mut bad = Vec::new();
    for &(src, legacy, msg, line, col) in cases {
        let opts = CompileOptions {
            experimental_decorators: legacy,
            ..CompileOptions::script("t.ts").ts()
        };
        let mut heap = Heap::new().unwrap();
        let e = match compile(&mut heap, src.as_bytes(), &opts) {
            Ok(_) => {
                bad.push(alloc::format!("{src:?} compiled"));
                continue;
            }
            Err(e) => e,
        };
        if (e.message.as_str(), e.line, e.col) != (msg, line, col) {
            bad.push(alloc::format!(
                "{src:?}: got {:?} {}:{}, want {msg:?} {line}:{col}",
                e.message,
                e.line,
                e.col
            ));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// Without the flag, TypeScript is a syntax error.
#[test]
fn ts_needs_the_flag() {
    for src in [
        "let x: number = 1;",
        "function f<T>(x: T) {}",
        "type T = number;",
    ] {
        let mut heap = Heap::new().unwrap();
        assert!(compile(&mut heap, src.as_bytes(), &CompileOptions::script("t.js")).is_err());
    }
}

fn strip(src: &str) -> String {
    let mut heap = Heap::new().unwrap();
    let out = strip_types(
        &mut heap,
        src.as_bytes(),
        &CompileOptions::async_body("t.ts", &[]),
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

/// `strip_types` blanks exactly the erased ranges (checked with ts-blank-space).
#[test]
fn ts_strip() {
    let cases: &[(&str, &str)] = &[
        ("let x: number = 1;", "let x         = 1;"),
        (
            "function f<T>(a: T, b?: number): T { return a }",
            "function f   (a   , b         )    { return a }",
        ),
        (
            "const y = f<string>(x!) as any;",
            "const y = f        (x )       ;",
        ),
        ("type T = {\n  a: 1\n};", "          \n      \n  "),
        ("let s: 'é𝄞' = 1;", "let s        = 1;"),
        // Parameter decorators stay (ts-blank-space).
        (
            "class C { m(@dec private p: number) {} }",
            "class C { m(@dec         p        ) {} }",
        ),
        // A parameter property is not erasable (it assigns `this.x`): it stays, and the
        // text is not JavaScript.
        (
            "class P { constructor(public x: number, readonly y) {} }",
            "class P { constructor(public x        , readonly y) {} }",
        ),
    ];
    let mut bad = Vec::new();
    for &(src, want) in cases {
        let got = strip(src);
        if got != want {
            bad.push(alloc::format!("{src:?}\n  got:  {got:?}\n  want: {want:?}"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// tests/strip-types: `strip_types` matches ts-blank-space byte for byte (the fast part
/// of the strip-types oracle; the full one is scripts/strip-types/oracle.sh).
#[test]
fn ts_strip_corpus() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/strip-types");
    let mut cases: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ts"))
        .collect();
    cases.sort();
    assert!(!cases.is_empty());
    let mut bad = Vec::new();
    for ts in cases {
        let src = std::fs::read(&ts).unwrap();
        let want = std::fs::read(ts.with_extension("js")).unwrap();
        let mut heap = Heap::new().unwrap();
        match strip_types(&mut heap, &src, &CompileOptions::async_body("t.ts", &[])) {
            Ok(got) if got == want => {}
            Ok(got) => bad.push(alloc::format!(
                "{}:\n--- ts-blank-space\n{}\n--- djs\n{}",
                ts.display(),
                String::from_utf8_lossy(&want),
                String::from_utf8_lossy(&got)
            )),
            Err(e) => bad.push(alloc::format!("{}: {e}", ts.display())),
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
