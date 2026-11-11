//! Compiler tests: disassembly snapshots (`snapshots/*.js` → `*.disasm`) and errors.
//!
//! Regenerate the snapshots with `DJS_UPDATE_SNAPSHOTS=1 cargo test -p djs-core
//! compiler` and review the diff.

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;
use std::path::PathBuf;

use super::*;
use crate::bytecode::template::disasm_template;
use crate::heap::Heap;

const BODY_PARAMS: &[&str] = &["tools", "console"];

fn compile_src(heap: &mut Heap, src: &str) -> Result<Ref, CompileError> {
    let opts = if src.starts_with("// goal: body") {
        CompileOptions::async_body("test.js", BODY_PARAMS)
    } else {
        CompileOptions::script("test.js")
    };
    compile(heap, src.as_bytes(), &opts)
}

#[test]
fn snapshots() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/compiler/snapshots");
    let update = std::env::var_os("DJS_UPDATE_SNAPSHOTS").is_some();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "js"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    let mut failed = Vec::new();
    for js in files {
        let src = std::fs::read_to_string(&js).unwrap();
        let mut heap = Heap::new().unwrap();
        let text = if src.starts_with("// goal: module") {
            match compile_module(
                &mut heap,
                src.as_bytes(),
                &CompileOptions::module("test.js"),
            ) {
                Ok((t, meta)) => {
                    let words: Vec<u32> = (0..heap.u32_array_len(meta.0))
                        .map(|i| heap.u32_array_get(meta.0, i))
                        .collect();
                    let m = module_meta::ModuleMeta::decode(&words).unwrap();
                    let name = |a: crate::heap::Atom| {
                        if a.is_null() {
                            String::from("*")
                        } else {
                            heap.atom_name(a)
                        }
                    };
                    let mut out = disasm_template(&heap, t);
                    out.push_str("module:\n");
                    for (i, &(n, k, aux)) in m.env.iter().enumerate() {
                        out.push_str(&alloc::format!(
                            "  env {i} {} kind {k} aux {aux}\n",
                            name(n)
                        ));
                    }
                    for r in &m.requests {
                        out.push_str(&alloc::format!("  request {:?}", name(r.specifier)));
                        if !r.ty.is_null() {
                            out.push_str(&alloc::format!(" type {:?}", name(r.ty)));
                        }
                        if !r.unsupported.is_null() {
                            out.push_str(&alloc::format!(" unsupported {:?}", name(r.unsupported)));
                        }
                        out.push('\n');
                    }
                    for &(e, r, n) in &m.imports {
                        out.push_str(&alloc::format!(
                            "  import env {e} request {r} {:?}\n",
                            name(n)
                        ));
                    }
                    for &(n, e) in &m.local {
                        out.push_str(&alloc::format!("  export {:?} env {e}\n", name(n)));
                    }
                    for &(n, r, i) in &m.indirect {
                        out.push_str(&alloc::format!(
                            "  export {:?} request {r} {:?}\n",
                            name(n),
                            name(i)
                        ));
                    }
                    for r in &m.star {
                        out.push_str(&alloc::format!("  export * request {r}\n"));
                    }
                    out
                }
                Err(e) => alloc::format!("error: {e}\n"),
            }
        } else {
            match compile_src(&mut heap, &src) {
                Ok(t) => disasm_template(&heap, t),
                Err(e) => alloc::format!("error: {e}\n"),
            }
        };
        let expected_path = js.with_extension("disasm");
        if update {
            std::fs::write(&expected_path, &text).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&expected_path).unwrap_or_default();
        if expected != text {
            failed.push(alloc::format!(
                "{}:\n--- expected\n{expected}\n--- got\n{text}",
                js.display()
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "snapshot mismatch (DJS_UPDATE_SNAPSHOTS=1 to update):\n{}",
        failed.join("\n")
    );
}

fn err(src: &str) -> CompileError {
    let mut heap = Heap::new().unwrap();
    match compile_src(&mut heap, src) {
        Ok(_) => panic!("{src:?} compiled"),
        Err(e) => e,
    }
}

/// `(source, message, line, column)`; messages and positions follow qjs where it has
/// one (checked with `qjs` on `"use strict";` + source, column shifted by 13).
#[test]
fn errors() {
    let cases: &[(&str, &str, u32, u32)] = &[
        ("1 +", "unexpected token in expression: ''", 1, 4),
        (
            "let x = 1; let x = 2;",
            "invalid redefinition of lexical identifier",
            1,
            18,
        ),
        (
            "var x; let x;",
            "invalid redefinition of global identifier",
            1,
            13,
        ),
        (
            "// goal: body\nvar x; let x;",
            "invalid redefinition of a variable",
            2,
            13,
        ),
        (
            "let x; { var x; }",
            "invalid redefinition of lexical identifier",
            1,
            15,
        ),
        (
            "{ let x; var x; }",
            "invalid redefinition of lexical identifier",
            1,
            15,
        ),
        (
            "let a, a;",
            "invalid redefinition of lexical identifier",
            1,
            9,
        ),
        ("const x;", "missing initializer for const variable", 1, 8),
        ("1 = 2", "invalid assignment left-hand side", 1, 3),
        ("a + 1 = 2", "invalid assignment left-hand side", 1, 7),
        ("({a}) = 1", "invalid assignment left-hand side", 1, 7),
        ("1++", "invalid increment/decrement operand", 1, 2),
        ("++f()", "invalid increment/decrement operand", 1, 6),
        (
            "delete x",
            "cannot delete a direct reference in strict mode",
            1,
            1,
        ),
        ("var eval", "invalid variable name in strict mode", 1, 5),
        ("eval = 1", "invalid lvalue in strict mode", 1, 1),
        ("arguments++", "invalid lvalue in strict mode", 1, 1),
        ("var let", "variable name expected", 1, 5),
        ("var yield", "variable name expected", 1, 5),
        ("var \\u0069f", "variable name expected", 1, 5),
        ("let let = 1", "variable name expected", 1, 5),
        ("var if", "variable name expected", 1, 5),
        ("yield = 1", "unexpected 'yield' keyword", 1, 1),
        (
            "x = l\\u0065t",
            "unexpected token in expression: 'let'",
            1,
            5,
        ),
        (
            "-a ** 2",
            "unparenthesized unary expression can't appear on the left-hand side of '**'",
            1,
            4,
        ),
        ("a ?? b || c", "cannot mix ?? with && or ||", 1, 3),
        ("a || b ?? c", "cannot mix ?? with && or ||", 1, 8),
        ("throw\n1", "line terminator not allowed after throw", 2, 1),
        ("return 1", "return not in a function", 1, 1),
        ("break", "break must be inside loop or switch", 1, 1),
        ("continue", "continue must be inside loop", 1, 1),
        ("a: { continue a; }", "break/continue label not found", 1, 6),
        (
            "while (1) { break b; }",
            "break/continue label not found",
            1,
            13,
        ),
        ("a: a: 1", "duplicate label name", 1, 4),
        (
            "if (1) let x = 1",
            "lexical declarations can't appear in single-statement context",
            1,
            8,
        ),
        (
            "while (1) const x = 1;",
            "lexical declarations can't appear in single-statement context",
            1,
            11,
        ),
        (
            "new.target",
            "new.target only allowed within functions",
            1,
            1,
        ),
        ("super.x", "'super' is only valid in a method", 1, 6),
        ("with (a) {}", "invalid keyword: with", 1, 1),
        ("export var x", "unsupported keyword: export", 1, 1),
        (
            "switch (1) { default: default: }",
            "duplicate default",
            1,
            23,
        ),
        ("x\n=>1", "unexpected token in expression: '=>'", 2, 1),
        ("`${`", "unexpected end of string", 1, 4),
        ("`\\x`", "malformed escape sequence in string literal", 1, 2),
        ("let x = 1 2", "expecting ';'", 1, 11),
        ("f(1, 2", "expecting ')'", 1, 7),
        ("({a = 1})", "expecting '}'", 1, 5),
        ("({a: 1} = c)", "invalid destructuring target", 1, 7),
        (
            "'\\08'",
            "octal escape sequences are not allowed in strict mode",
            1,
            2,
        ),
        (
            "x = 08",
            "octal literals are deprecated in strict mode",
            1,
            5,
        ),
        ("a?.b = 1", "invalid assignment left-hand side", 1, 6),
        (
            "new a?.b()",
            "new keyword cannot be used with an optional chain",
            1,
            6,
        ),
        // Functions (0024).
        (
            "function f(a, a) {}",
            "duplicate argument names not allowed in this context",
            1,
            16,
        ),
        (
            "(a, a) => 1",
            "duplicate argument names not allowed in this context",
            1,
            6,
        ),
        (
            "function f(a = 1) { \"use strict\" }",
            "\"use strict\" not allowed in function with default or destructuring parameter",
            1,
            21,
        ),
        (
            "function eval() {}",
            "invalid function name in strict code",
            1,
            10,
        ),
        (
            "function f(eval) {}",
            "invalid argument name in strict code",
            1,
            12,
        ),
        (
            "(arguments) => 1",
            "invalid argument name in strict code",
            1,
            2,
        ),
        ("function f(...a, b) {}", "expecting ')'", 1, 16),
        ("function f(...a = 1) {}", "expecting ')'", 1, 17),
        ("function f(,) {}", "missing formal parameter", 1, 12),
        (
            "() => new.target",
            "new.target only allowed within functions",
            1,
            7,
        ),
        (
            "function f() { let a; var a }",
            "invalid redefinition of lexical identifier",
            1,
            29,
        ),
        (
            "function f(a) { let a }",
            "invalid redefinition of parameter name",
            1,
            23,
        ),
        (
            "{ function f(){} let f }",
            "invalid redefinition of lexical identifier",
            1,
            24,
        ),
        (
            "{ let f; function f(){} }",
            "invalid redefinition of lexical identifier",
            1,
            19,
        ),
        (
            "if (1) function f(){}",
            "function declarations can't appear in single-statement context",
            1,
            8,
        ),
        (
            "label: function f(){}",
            "function declarations can't appear in single-statement context",
            1,
            8,
        ),
        ("function () {}", "function name expected", 1, 10),
        (
            "function f() { x\n=> 1 }",
            "unexpected token in expression: '=>'",
            2,
            1,
        ),
        // Exceptions and lexical scoping (0025).
        ("try {}", "expecting catch or finally", 1, 7),
        ("try { throw 1 }", "expecting catch or finally", 1, 16),
        ("try {} catch () {}", "identifier expected", 1, 15),
        (
            "try {} catch (eval) {}",
            "invalid variable name in strict mode",
            1,
            15,
        ),
        ("try {} catch (e, f) {}", "expecting ')'", 1, 16),
        (
            "try {} catch (e) { let e }",
            "invalid redefinition of lexical identifier",
            1,
            26,
        ),
        (
            "try {} catch (e) { function e(){} }",
            "invalid redefinition of lexical identifier",
            1,
            29,
        ),
        (
            "try {} catch ([a, a]) {}",
            "invalid redefinition of lexical identifier",
            1,
            20,
        ),
        (
            "try {} catch ([e]) { var e }",
            "invalid redefinition of lexical identifier",
            1,
            28,
        ),
        (
            "switch (1) { case 1: let a; case 2: let a; }",
            "invalid redefinition of lexical identifier",
            1,
            42,
        ),
        (
            "function f(){} let f",
            "invalid redefinition of global identifier",
            1,
            21,
        ),
        (
            "{ function f(){} function f(){} }",
            "invalid redefinition of lexical identifier",
            1,
            27,
        ),
        (
            "for (let i;;) { var i }",
            "invalid redefinition of lexical identifier",
            1,
            23,
        ),
        (
            "try { break } finally {}",
            "break must be inside loop or switch",
            1,
            7,
        ),
        // Classes (0040); messages and positions as qjs (qjs reports undefined private
        // names without a position; it accepts `class eval` and `delete this?.#x`).
        (
            "class A { constructor(){} constructor(){} }",
            "property constructor appears more than once",
            1,
            38,
        ),
        ("class A { #constructor(){} }", "invalid method name", 1, 23),
        ("class A { #constructor }", "invalid method name", 1, 24),
        (
            "class A { get constructor(){} }",
            "invalid method name",
            1,
            26,
        ),
        (
            "class A { async constructor(){} }",
            "invalid method name",
            1,
            28,
        ),
        ("class A { *constructor(){} }", "invalid method name", 1, 23),
        ("class A { constructor = 1 }", "invalid field name", 1, 23),
        (
            "class A { static prototype(){} }",
            "invalid method name",
            1,
            27,
        ),
        (
            "class A { static prototype = 1 }",
            "invalid method name",
            1,
            28,
        ),
        (
            "class A { static get prototype(){} }",
            "invalid method name",
            1,
            31,
        ),
        (
            "class A { m(){ super(); } }",
            "super() is only valid in a derived class constructor",
            1,
            21,
        ),
        (
            "class A extends B { m(){ super(); } }",
            "super() is only valid in a derived class constructor",
            1,
            31,
        ),
        (
            "class A { constructor(){ super(); } }",
            "super() is only valid in a derived class constructor",
            1,
            31,
        ),
        (
            "class A extends B { x = super(); }",
            "super() is only valid in a derived class constructor",
            1,
            30,
        ),
        (
            "class A extends B { static { super(); } }",
            "super() is only valid in a derived class constructor",
            1,
            35,
        ),
        (
            "function f(){ super.x }",
            "'super' is only valid in a method",
            1,
            20,
        ),
        (
            "class A { #x; m(){ delete this.#x } }",
            "cannot delete a private class field",
            1,
            35,
        ),
        (
            "class C { @dec static { } }",
            "decorators are not valid here",
            1,
            23,
        ),
        (
            "class C { @dec constructor() {} }",
            "decorators are not valid here",
            1,
            27,
        ),
        (
            "@dec function f() {}",
            "decorators must be followed by a class",
            1,
            6,
        ),
        (
            "x = @dec 1",
            "decorators must be followed by a class",
            1,
            10,
        ),
        (
            "if (1) @dec class C {}",
            "class declarations can't appear in single-statement context",
            1,
            8,
        ),
        ("class E { @1 m() {} }", "invalid decorator", 1, 11),
        ("class E { @this.x m() {} }", "invalid decorator", 1, 11),
        ("class E { @dec; }", "invalid property name", 1, 15),
        ("class C { @dec[0] m() {} }", "expecting ';'", 1, 19),
        (
            "class E { accessor #x; #x; }",
            "private class field is already defined",
            1,
            26,
        ),
        (
            "class E { accessor #x; get #x() {} }",
            "private class field is already defined",
            1,
            30,
        ),
        ("class E { accessor m() {} }", "expecting ';'", 1, 21),
        (
            "class E { accessor constructor; }",
            "invalid field name",
            1,
            31,
        ),
        (
            "class E { @dec accessor #constructor; }",
            "invalid method name",
            1,
            37,
        ),
        (
            "class A { #x; #x }",
            "private class field is already defined",
            1,
            18,
        ),
        (
            "class A { #x; #x(){} }",
            "private class field is already defined",
            1,
            22,
        ),
        (
            "class A { get #x(){} get #x(){} }",
            "private class field is already defined",
            1,
            28,
        ),
        (
            "class A { get #x(){} static set #x(v){} }",
            "private class field is already defined",
            1,
            35,
        ),
        (
            "class A { x = arguments }",
            "'arguments' identifier is not allowed in class field initializer",
            1,
            15,
        ),
        (
            "class A { x = () => arguments }",
            "'arguments' identifier is not allowed in class field initializer",
            1,
            21,
        ),
        (
            "class A { static { arguments } }",
            "'arguments' identifier is not allowed in class field initializer",
            1,
            20,
        ),
        (
            "class A { static { await } }",
            "unexpected 'await' keyword",
            1,
            20,
        ),
        (
            "class A { static { await 1 } }",
            "unexpected 'await' keyword",
            1,
            20,
        ),
        (
            "class A { static { return } }",
            "return in a static initializer block",
            1,
            20,
        ),
        (
            "class A { static { var await } }",
            "variable name expected",
            1,
            24,
        ),
        (
            "async function f(){ class A { x = await 1 } }",
            "expecting ';'",
            1,
            41,
        ),
        (
            "function* g(){ class A { x = yield } }",
            "unexpected 'yield' keyword",
            1,
            30,
        ),
        (
            "function* g(){ class A { x = yield 1 } }",
            "unexpected 'yield' keyword",
            1,
            30,
        ),
        (
            "class A { #x; m(){ #x } }",
            "unexpected token in expression: '#x'",
            1,
            20,
        ),
        (
            "class A { #x; m(){ #x + 1 } }",
            "unexpected token in expression: '#x'",
            1,
            20,
        ),
        (
            "class A { #x; m(){ 1 + #x in this } }",
            "unexpected token in expression: '#x'",
            1,
            24,
        ),
        (
            "class A { #x; m(){ #x in #x in this } }",
            "unexpected token in expression: '#x'",
            1,
            26,
        ),
        ("class let {}", "class statement requires a name", 1, 7),
        ("class yield {}", "class statement requires a name", 1, 7),
        (
            "async function f(){ class await {} }",
            "class statement requires a name",
            1,
            27,
        ),
        ("class A { m(){ super } }", "invalid use of 'super'", 1, 22),
        (
            "class A { m(){ super?.x } }",
            "invalid use of 'super'",
            1,
            21,
        ),
        (
            "class A extends B { constructor(){ super?.() } }",
            "invalid use of 'super'",
            1,
            41,
        ),
        ("class {}", "class statement requires a name", 1, 7),
        ("class A { x y }", "expecting ';'", 1, 13),
        (
            "class A { 'constructor'(){} constructor(){} }",
            "property constructor appears more than once",
            1,
            40,
        ),
        (
            "class A { static #a; #a }",
            "private class field is already defined",
            1,
            25,
        ),
        (
            "x = { m(){ super(); } }",
            "super() is only valid in a derived class constructor",
            1,
            17,
        ),
        (
            "class A { static async { } }",
            "invalid property name",
            1,
            24,
        ),
        (
            "class A { static x = arguments }",
            "'arguments' identifier is not allowed in class field initializer",
            1,
            22,
        ),
        (
            "class A { static constructor = 1 }",
            "invalid field name",
            1,
            30,
        ),
        (
            "class A { static \"constructor\" }",
            "invalid field name",
            1,
            32,
        ),
        ("class A { static prototype }", "invalid method name", 1, 28),
        (
            "class A { static async {} }",
            "invalid property name",
            1,
            24,
        ),
        ("class A { x = 1 y = 2 }", "expecting ';'", 1, 17),
        ("class A { get x }", "invalid property name", 1, 17),
        (
            "class A { static async *[x] = 1 }",
            "invalid property name",
            1,
            29,
        ),
        (
            "class A { m() { new super() } }",
            "super() is only valid in a derived class constructor",
            1,
            26,
        ),
        (
            "class A extends B { constructor() { super.#x } }",
            "private class field forbidden after super",
            1,
            43,
        ),
        (
            "class A { #x; m() { (#x) in this } }",
            "unexpected token in expression: '#x'",
            1,
            22,
        ),
        (
            "class A { set #x(a, b) {} }",
            "invalid number of arguments for getter or setter",
            1,
            22,
        ),
        (
            "class A { get x(a) {} }",
            "invalid number of arguments for getter or setter",
            1,
            18,
        ),
        (
            "class A { [super.x] }",
            "'super' is only valid in a method",
            1,
            17,
        ),
        (
            "class A extends B { x = () => super() }",
            "super() is only valid in a derived class constructor",
            1,
            36,
        ),
        (
            "function f() { class A { static { return 1 } } }",
            "return in a static initializer block",
            1,
            35,
        ),
        (
            "class A { static { yield } }",
            "unexpected 'yield' keyword",
            1,
            20,
        ),
        (
            "class A { x = class { y = arguments } }",
            "'arguments' identifier is not allowed in class field initializer",
            1,
            27,
        ),
        (
            "class A { m() { this.#x } }",
            "undefined private field '#x'",
            1,
            22,
        ),
        (
            "class eval {}",
            "invalid variable name in strict mode",
            1,
            7,
        ),
        (
            "class A { #x; m() { delete this?.#x } }",
            "cannot delete a private class field",
            1,
            37,
        ),
        // Destructuring (0036); messages and positions as qjs.
        (
            "let [a, ...b,] = c",
            "rest element must be the last one",
            1,
            13,
        ),
        (
            "let [...a = 1] = c",
            "rest element must be the last one",
            1,
            11,
        ),
        ("[...a, b] = c", "rest element must be the last one", 1, 6),
        (
            "let {...a, b} = c",
            "assignment rest property must be last",
            1,
            10,
        ),
        (
            "({...a,} = b)",
            "assignment rest property must be last",
            1,
            7,
        ),
        ("let {...{a}} = c", "invalid destructuring target", 1, 9),
        ("({...{a}} = c)", "invalid destructuring target", 1, 9),
        ("[a, b + 1] = c", "expecting ','", 1, 7),
        ("[({a})] = 1", "invalid destructuring target", 1, 7),
        ("[(a = 1)] = 1", "invalid destructuring target", 1, 9),
        ("[f()] = 1", "invalid destructuring target", 1, 5),
        ("({a: f()} = 1)", "invalid destructuring target", 1, 9),
        ("({eval} = 1)", "invalid destructuring target", 1, 7),
        ("[arguments] = 1", "invalid lvalue in strict mode", 1, 11),
        ("let {a: eval} = 1", "invalid destructuring target", 1, 9),
        ("let {a}", "variable name expected", 1, 5),
        ("const [a];", "variable name expected", 1, 7),
        (
            "let [a, a] = c",
            "invalid redefinition of lexical identifier",
            1,
            10,
        ),
        (
            "({ __proto__: 1, __proto__: 2 } = {})",
            "invalid destructuring target",
            1,
            16,
        ),
        // for-in/of (0029); messages and positions as qjs.
        (
            "for (x) ;",
            "expected 'of' or 'in' in for control expression",
            1,
            7,
        ),
        (
            "for (a + 1 in {}) ;",
            "expected 'of' or 'in' in for control expression",
            1,
            8,
        ),
        (
            "for (x++ of []) ;",
            "expected 'of' or 'in' in for control expression",
            1,
            7,
        ),
        (
            "for (let x, y of []) ;",
            "expected 'of' or 'in' in for control expression",
            1,
            11,
        ),
        (
            "for (let x = 1 in {}) ;",
            "a declaration in the head of a for-in loop can't have an initializer",
            1,
            16,
        ),
        (
            "for (var x = 1 of []) ;",
            "a declaration in the head of a for-of loop can't have an initializer",
            1,
            16,
        ),
        (
            "for (a = 1 of []) ;",
            "a declaration in the head of a for-of loop can't have an initializer",
            1,
            12,
        ),
        ("for (x of a, b) ;", "expecting ')'", 1, 12),
        ("for (x of y", "expecting ')'", 1, 12),
        ("for (x of ;", "expecting ';'", 1, 8),
        (
            "for (async of []) ;",
            "'for of' expression cannot start with 'async'",
            1,
            6,
        ),
        (
            "for (a?.b of []) ;",
            "invalid for in/of left hand-side",
            1,
            11,
        ),
        (
            "for (this of []) ;",
            "invalid for in/of left hand-side",
            1,
            11,
        ),
        (
            "for (new A of []) ;",
            "invalid for in/of left hand-side",
            1,
            12,
        ),
        (
            "for (const x of []) const y = 1;",
            "lexical declarations can't appear in single-statement context",
            1,
            21,
        ),
        (
            "for (let x of []) { var x; }",
            "invalid redefinition of lexical identifier",
            1,
            26,
        ),
        (
            "let x; for (var x of []) ;",
            "invalid redefinition of lexical identifier",
            1,
            19,
        ),
        (
            "for (let [a] = 1 of b);",
            "a declaration in the head of a for-of loop can't have an initializer",
            1,
            18,
        ),
        ("for await (x of y) ;", "expecting '('", 1, 5),
        // Regex literals (0033): the pattern's early errors at the literal, like qjs.
        ("x = /a(/", "expecting ')'", 1, 5),
        ("x = 1;\n  y = /[b-a]/u", "invalid class range", 2, 7),
        ("x = /a/gg", "invalid regular expression flags", 1, 5),
        (
            "a?.b`x`",
            "template literal cannot appear in an optional chain",
            1,
            5,
        ),
        // Objects (0027).
        (
            "({ get x(a) {} })",
            "invalid number of arguments for getter or setter",
            1,
            11,
        ),
        (
            "({ set x() {} })",
            "invalid number of arguments for getter or setter",
            1,
            10,
        ),
        (
            "({ set x(...a) {} })",
            "invalid number of arguments for getter or setter",
            1,
            14,
        ),
        (
            "({ __proto__: 1, __proto__: 2 })",
            "duplicate __proto__ property name",
            1,
            31,
        ),
        (
            "({ '__proto__': 1, __proto__: 2 })",
            "duplicate __proto__ property name",
            1,
            33,
        ),
        ("({ #x: 1 })", "invalid property name", 1, 4),
        ("({ get #x() {} })", "invalid property name", 1, 8),
        // Async functions and generators (0041); messages and positions as qjs.
        (
            "async function f(a = await 1) {}",
            "await in default expression",
            1,
            22,
        ),
        (
            "async function f() { var await; }",
            "variable name expected",
            1,
            26,
        ),
        ("function f() { await 1; }", "expecting ';'", 1, 22),
        (
            "async function f() { function g() { await 1; } }",
            "expecting ';'",
            1,
            43,
        ),
        (
            "async (a = await 1) => a;",
            "await in default expression",
            1,
            12,
        ),
        ("async (await) => 1;", "missing formal parameter", 1, 8),
        ("async await => 1;", "expecting '('", 1, 7),
        (
            "function* g(a = yield) {}",
            "yield in default expression",
            1,
            17,
        ),
        (
            "function* g() { var yield; }",
            "variable name expected",
            1,
            21,
        ),
        (
            "function* g() { function h() { yield 1; } }",
            "unexpected 'yield' keyword",
            1,
            32,
        ),
        (
            "function* g() { () => yield 1; }",
            "unexpected 'yield' keyword",
            1,
            23,
        ),
        (
            "function* g() { yield* ; }",
            "unexpected token in expression: ';'",
            1,
            24,
        ),
        (
            "function f() { for await (x of y) ; }",
            "expecting '('",
            1,
            20,
        ),
        (
            "var o = { async get x() {} };",
            "invalid property name",
            1,
            21,
        ),
        (
            "async function f() { await => 1; }",
            "unexpected token in expression: '=>'",
            1,
            28,
        ),
        (
            "function* g() { yield => 1; }",
            "unexpected token in expression: '=>'",
            1,
            23,
        ),
        (
            "let f = async () => await;",
            "unexpected token in expression: ';'",
            1,
            26,
        ),
        (
            "function* g() { yield = 1; }",
            "unexpected token in expression: '='",
            1,
            23,
        ),
        (
            "async function f() { (a = await 1) => 1; }",
            "unexpected 'await' keyword",
            1,
            27,
        ),
        (
            "function* g() { (a = yield) => 1; }",
            "unexpected 'yield' keyword",
            1,
            22,
        ),
        (
            "async function f() { return () => await 1; }",
            "expecting ';'",
            1,
            41,
        ),
        (
            "async function f() { return async function await() {}; }",
            "expecting '('",
            1,
            44,
        ),
        (
            "async function f() { let await; }",
            "variable name expected",
            1,
            26,
        ),
        ("async function f() { ({await}) }", "expecting ':'", 1, 29),
        (
            "async function f() { await: ; }",
            "unexpected token in expression: ':'",
            1,
            27,
        ),
        (
            "var await; async function f() { await.x }",
            "unexpected token in expression: '.'",
            1,
            38,
        ),
        (
            "async function f() { new await 1 }",
            "unexpected token in expression: 'await'",
            1,
            26,
        ),
        ("* 2; }", "unexpected token in expression: '*'", 1, 1),
        (
            "function* g() { yield ? 1 : 2; }",
            "unexpected token in expression: '?'",
            1,
            23,
        ),
        (
            "function* g() { yield in x; }",
            "unexpected token in expression: 'in'",
            1,
            23,
        ),
        (
            "function* g() { x = yield in y; }",
            "unexpected token in expression: 'in'",
            1,
            27,
        ),
        (
            "function* g() { let yield; }",
            "variable name expected",
            1,
            21,
        ),
        (
            "function* g() { function* yield() {} }",
            "function name expected",
            1,
            27,
        ),
        ("function* yield() {}", "function name expected", 1, 11),
        (
            "function *g() { async function f() { yield 1 } }",
            "unexpected 'yield' keyword",
            1,
            38,
        ),
        (
            "async function f() { yield 1 }",
            "unexpected 'yield' keyword",
            1,
            22,
        ),
        ("function* g() { await 1 }", "expecting ';'", 1, 23),
        (
            "async function f(...await) {}",
            "missing formal parameter",
            1,
            21,
        ),
        (
            "async function f() { for await (x in y) ; }",
            "'for await' loop should be used with 'of'",
            1,
            35,
        ),
        (
            "async function f() { for await (;;) ; }",
            "unexpected token in expression: ';'",
            1,
            33,
        ),
        (
            "function* g() { yield* }",
            "unexpected token in expression: '}'",
            1,
            24,
        ),
        (
            "// goal: body\nawait => 1",
            "unexpected token in expression: '=>'",
            2,
            7,
        ),
        ("// goal: body\nvar await", "variable name expected", 2, 5),
        // `import()` (0058).
        ("import.meta", "import.meta only valid in module code", 1, 8),
        ("import.foo", "meta expected", 1, 8),
        ("import(a, b, c)", "expecting ')'", 1, 14),
        ("new import(a)", "invalid use of 'import()'", 1, 5),
        ("x = import", "expecting '('", 1, 11),
        // `using` declarations (0058).
        (
            "using x = null;",
            "using declaration not allowed here",
            1,
            7,
        ),
        (
            "switch (1) { case 1: using x = null; }",
            "using declaration not allowed here",
            1,
            28,
        ),
        (
            "{ using x; }",
            "missing initializer for using declaration",
            1,
            10,
        ),
        (
            "{ using x = null, [y] = null; }",
            "binding patterns are not allowed in using declarations",
            1,
            19,
        ),
        (
            "for (using x in {}) {}",
            "using declaration not allowed in for-in loop",
            1,
            14,
        ),
        (
            "if (1) using x = null;",
            "lexical declarations can't appear in single-statement context",
            1,
            8,
        ),
        (
            "async function f() { for (await using x in {}) {} }",
            "using declaration not allowed in for-in loop",
            1,
            41,
        ),
        ("{ using let = null; }", "expecting ';'", 1, 9),
        (
            "async function f() { await using await = null; }",
            "expecting ';'",
            1,
            34,
        ),
    ];
    let mut bad = Vec::new();
    for &(src, msg, line, col) in cases {
        let e = err(src);
        if (e.message.as_str(), e.line, e.col) != (msg, line, col)
            || e.kind != CompileErrorKind::Syntax
        {
            bad.push(alloc::format!(
                "{src:?}: got {:?} at {}:{}, want {msg:?} at {line}:{col}",
                e.message,
                e.line,
                e.col
            ));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

#[test]
fn display() {
    let e = err("1 +");
    assert_eq!(
        alloc::format!("{e}"),
        "SyntaxError: unexpected token in expression: '' at 1:4"
    );
    let _: String = e.message;
}

/// Nested constructs, `n` levels deep.
fn nested(kind: &str, n: usize) -> String {
    let (open, mid, close) = match kind {
        "paren" => ("(", "1", ")"),
        "array" => ("[", "1", "]"),
        "object" => ("{a:", "1", "}"),
        "unary" => ("!", "x", ""),
        "ternary" => ("a ? b : ", "c", ""),
        "block" => ("{", "", "}"),
        "if" => ("if (a) ", "x;", ""),
        "assign" => ("a = ", "1", ""),
        "member" => ("", "a", ".b"),
        "new" => ("new ", "X", ""),
        _ => unreachable!(),
    };
    let mut s = String::from("x = ");
    if matches!(kind, "block" | "if") {
        s.clear();
    }
    for _ in 0..n {
        s.push_str(open);
    }
    s.push_str(mid);
    for _ in 0..n {
        s.push_str(close);
    }
    s
}

const KINDS: &[&str] = &[
    "paren", "array", "object", "unary", "ternary", "block", "if", "assign", "member", "new",
];

/// The deepest nesting of each construct that compiles: run on a thread with the wasm
/// default stack (1 MiB; debug builds get more, their frames are larger), so hitting
/// the depth limit is a SyntaxError, never a stack overflow (docs/compiler.md 2.4).
#[test]
fn depth_limit_fits_wasm_stack() {
    let stack = match std::env::var("DJS_DEPTH_STACK") {
        // For measuring the headroom: DJS_DEPTH_STACK=262144 cargo test --release ...
        Ok(s) => s.parse().unwrap(),
        Err(_) if cfg!(debug_assertions) => 8 << 20,
        Err(_) => 1 << 20,
    };
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(|| {
            for kind in KINDS {
                // Find the largest n that compiles, by doubling then bisecting.
                let ok = |n: usize| {
                    let mut heap = Heap::new().unwrap();
                    compile(
                        &mut heap,
                        nested(kind, n).as_bytes(),
                        &CompileOptions::script("t"),
                    )
                    .is_ok()
                };
                let (mut lo, mut hi) = (1, 2);
                while ok(hi) && hi < 1 << 13 {
                    lo = hi;
                    hi *= 2;
                }
                while hi - lo > 1 {
                    let m = (lo + hi) / 2;
                    if ok(m) {
                        lo = m;
                    } else {
                        hi = m;
                    }
                }
                std::println!("{kind}: max depth {lo}");
                // Chains that loop rather than recurse have no limit.
                if *kind == "member" {
                    assert!(lo >= 1 << 12, "{kind}: {lo}");
                    continue;
                }
                assert!(lo >= 150, "{kind}: {lo}");
                let mut heap = Heap::new().unwrap();
                let e = compile(
                    &mut heap,
                    nested(kind, hi).as_bytes(),
                    &CompileOptions::script("t"),
                )
                .unwrap_err();
                assert_eq!(e.message, "stack overflow", "{kind}");
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

fn test262_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DJS_TEST262") {
        return Some(dir.into());
    }
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    let common = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    let dir = common.parent()?.join("vendor/test262/test");
    dir.is_dir().then_some(dir)
}

fn collect_js(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_js(&path, out);
        } else if path.extension().is_some_and(|e| e == "js") {
            out.push(path);
        }
    }
}

/// Compiles every test262 test (without its harness). Must not panic. Lists the
/// non-negative tests that fail with something other than "not supported yet":
/// candidates for parser bugs. `cargo test -p djs-core --release compile_test262 --
/// --ignored --nocapture`
#[test]
#[ignore]
fn compile_test262() {
    let Some(root) = test262_dir() else {
        std::println!("vendor/test262 not found; skipping");
        return;
    };
    let mut files = Vec::new();
    collect_js(&root, &mut files);
    files.sort();
    let (mut ok, mut unsupported, mut negative, mut sloppy) = (0, 0, 0, 0);
    let mut other: Vec<String> = Vec::new();
    // Negative parse tests that compile: missing early errors.
    let mut missed: Vec<String> = Vec::new();
    for path in &files {
        let src = std::fs::read(path).unwrap();
        let text = String::from_utf8_lossy(&src);
        let mut heap = Heap::new().unwrap();
        let r = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
            compile(&mut heap, &src, &CompileOptions::script("t.js"))
        }));
        let Ok(r) = r else {
            panic!("compiler panicked on {}", path.display());
        };
        match r {
            Ok(_) if text.contains("phase: parse") => {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                missed.push(alloc::format!("{}", rel.display()));
            }
            Ok(_) => ok += 1,
            Err(e) if e.message.ends_with("not supported yet") => unsupported += 1,
            Err(_) if text.contains("negative:") => negative += 1,
            Err(_) if text.contains("noStrict") || text.contains("flags: [raw]") => sloppy += 1,
            // Modules are not supported (TESTING.md skips them).
            Err(_) if text.contains("module]") || text.contains(", module") => sloppy += 1,
            Err(e) => {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                other.push(alloc::format!(
                    "{}:{}:{}: {}",
                    rel.display(),
                    e.line,
                    e.col,
                    e.message
                ));
            }
        }
    }
    std::println!(
        "{} files: {ok} ok, {unsupported} not supported yet, {negative} negative, {sloppy} sloppy or module, {} other errors",
        files.len(),
        other.len()
    );
    for o in &other {
        std::println!("  {o}");
    }
    std::println!(
        "{} negative parse tests compile (missing early errors):",
        missed.len()
    );
    for m in &missed {
        std::println!("  missed {m}");
    }
}

// ---------------------------------------------------------------------------------
// End to end: compiled code on the interpreter (0015), in async-body mode with a host
// `console.log`. Expected outputs were checked against qjs.

/// Runs `src` as a codemode body with `console` as its parameter. Returns the printed
/// lines and the completion (`=> value` or `throw error`).
fn run_body(src: &str) -> String {
    run_body_opts(src, false)
}

/// [`run_body`], optionally as TypeScript.
pub(super) fn run_body_opts(src: &str, typescript: bool) -> String {
    let opts = CompileOptions {
        typescript,
        ..CompileOptions::async_body("e2e.js", &["console"])
    };
    run_opts(src, &opts)
}

/// Runs `src` compiled with `opts` (a Script or eval code gets `console` too, as an
/// ignored argument; it prints through the global `console` there).
pub(super) fn run_opts(src: &str, opts: &CompileOptions) -> String {
    use crate::vm::{Status, Vm, VmOptions};
    const LOG: u32 = 1;
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let console = vm.new_object().unwrap();
    let log = vm.new_host_function(LOG, "log").unwrap();
    vm.define(console, "log", log).unwrap();
    let global = vm.global();
    vm.define(global, "console", crate::value::Value::object(console))
        .unwrap();
    let template = match compile(vm.heap_mut(), src.as_bytes(), opts) {
        Ok(t) => t,
        Err(e) => return alloc::format!("compile error: {e}"),
    };
    let main = vm.instantiate(template).unwrap();
    let this = crate::value::Value::object(vm.global());
    vm.call(main, this, &[crate::value::Value::object(console)])
        .unwrap();
    let mut out = String::new();
    loop {
        match vm.run(1_000_000) {
            // The body is an async function: its result is a promise (D008).
            Status::Done(v) => {
                let line = match vm.promise_state(v) {
                    Some(Some(Ok(r))) => alloc::format!("=> {}", vm.display(r)),
                    Some(Some(Err(e))) => alloc::format!("throw {}", vm.display(e)),
                    Some(None) => String::from("pending"),
                    None => alloc::format!("=> {}", vm.display(v)),
                };
                out.push_str(&line);
                return out;
            }
            Status::Threw(e) => {
                let s = vm.display(e);
                out.push_str(&alloc::format!("throw {s}"));
                return out;
            }
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, LOG);
                let info = vm.host_call(call_id).unwrap();
                let parts: Vec<String> = info.args.iter().map(|&a| vm.display(a)).collect();
                out.push_str(&parts.join(" "));
                out.push('\n');
                vm.resolve_host_call(call_id, Ok(crate::value::Value::UNDEFINED))
                    .unwrap();
            }
            Status::OutOfFuel => return out + "out of fuel",
            Status::Idle => return out + "idle",
            Status::HostOutOfMemory => return out + "out of host memory",
        }
    }
}

#[test]
fn end_to_end() {
    let cases: &[(&str, &str)] = &[
        ("return 1 + 2 * 3", "=> 7"),
        ("let s = 'a'; s += 'b' + 1; return s", "=> ab1"),
        (
            "let x = 0; for (let i = 0; i < 10; i++) { if (i % 2) continue; x += i } return x",
            "=> 20",
        ),
        (
            "let n = 0; while (true) { n++; if (n > 4) break } return n",
            "=> 5",
        ),
        ("let n = 10; do { n -= 3 } while (n > 0); return n", "=> -2"),
        (
            "let i = 0; for (;;) { if (++i == 3) break } return i",
            "=> 3",
        ),
        ("let j = 0; for (; j < 3;) j++; return j", "=> 3"),
        (
            "let r = ''; outer: for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { if (j == 2) continue outer; if (i == 2) break outer; r += `${i}${j},` } } return r",
            "=> 00,01,10,11,",
        ),
        (
            "let out = ''; for (let v = 0; v < 5; v++) { switch (v) { case 0: out += 'zero '; case 1: out += 'one '; break; default: out += 'd' + v + ' '; break; case 3: out += 'three ' } } return out",
            "=> zero one one d2 three d4 ",
        ),
        (
            "let o = {a: 1, b: {c: 2}}; o.a += 10; o.b.c++; return o.a + o.b.c",
            "=> 14",
        ),
        (
            "let a = [1, 2, 3]; a[1] = 20; return a.length + a[1]",
            "=> 23",
        ),
        ("let o = null; return o?.x ?? 'none'", "=> none"),
        ("let o = {f: {g: 5}}; return o?.f?.g", "=> 5"),
        ("let x; x ??= 4; x ||= 5; x &&= x + 1; return x", "=> 5"),
        (
            "return typeof undeclared + ' ' + typeof 1 + ' ' + typeof 'a'",
            "=> undefined number string",
        ),
        ("let t = `a${1 + 1}b${'c'}`; return t", "=> a2bc"),
        (
            "console.log('hello', 1, true); console.log(); return",
            "hello 1 true\n\n=> undefined",
        ),
        (
            "let x = 1; { let x = 2; console.log(x) } return x",
            "2\n=> 1",
        ),
        ("return 2 ** 3 ** 2", "=> 512"),
        ("return (1, 2, 3)", "=> 3"),
        ("return 5 > 3 ? 'yes' : 'no'", "=> yes"),
        ("let a = 5; return -a + +'3' + ~1 + !a", "=> -4"),
        ("return 'x' in {x: 1}", "=> true"),
        ("x; let x = 1", "throw ReferenceError: x is not initialized"),
        ("const c = 1; c = 2", "throw TypeError: 'c' is read-only"),
        ("return nope", "throw ReferenceError: 'nope' is not defined"),
        ("throw 'boom'", "throw boom"),
        ("let n = 0; n++; n--; ++n; return n", "=> 1"),
        ("let a = 7; return a >> 1 | a << 2 & 0xff ^ 3", "=> 31"),
        ("return 0.1 + 0.2", "=> 0.30000000000000004"),
        ("return 1_000_000 + 0x10 + 0b11 + 0o7", "=> 1000026"),
    ];
    let mut bad = Vec::new();
    for &(src, want) in cases {
        let got = run_body(src);
        if got != want {
            bad.push(alloc::format!("{src}\n  got:  {got:?}\n  want: {want:?}"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// Functions and closures end to end (docs/compiler.md 5.3 to 5.9). Outputs checked
/// against qjs (`work/scratch/0024/qjs_check.py`).
#[test]
fn end_to_end_functions() {
    run_cases(&[
        (
            "function counter() { let n = 0; return () => ++n } const c = counter(); c(); c(); return c()",
            "=> 3",
        ),
        (
            "const fs = []; for (let i = 0; i < 3; i++) fs[i] = () => i; return fs[0]() + fs[1]() * 10 + fs[2]() * 100",
            "=> 210",
        ),
        (
            "var fs = []; for (var i = 0; i < 3; i++) fs[i] = () => i; return fs[0]()",
            "=> 3",
        ),
        (
            "const fs = []; for (let i = 0; i < 3; i++) { let j = i * 2; fs[i] = () => j } return fs[2]()",
            "=> 4",
        ),
        (
            "function a(x) { return function (y) { return (z) => x + y + z } } return a(1)(2)(3)",
            "=> 6",
        ),
        (
            "function fact(n) { return n <= 1 ? 1 : n * fact(n - 1) } return fact(10)",
            "=> 3628800",
        ),
        (
            "return isEven(10); function isEven(n) { return n == 0 ? true : isOdd(n - 1) } function isOdd(n) { return n == 0 ? false : isEven(n - 1) }",
            "=> true",
        ),
        (
            "function f(a, b = a * 2) { return a + b } return f(3) + f(3, 1)",
            "=> 13",
        ),
        (
            "function f(a = b, b) {} f()",
            "throw ReferenceError: b is not initialized",
        ),
        (
            "function f(x) { const g = () => x; x = 5; return g() } return f(1)",
            "=> 5",
        ),
        (
            "const c = 1; const f = () => { c = 2 }; f()",
            "throw TypeError: 'c' is read-only",
        ),
        (
            "{ function inner() { return 9 } } return typeof inner",
            "=> undefined",
        ),
        ("{ function inner() { return 9 } return inner() }", "=> 9"),
        (
            "const f = () => x; f(); let x = 1",
            "throw ReferenceError: x is not initialized",
        ),
        ("const add = (a, b) => a + b; return add(2, 3)", "=> 5"),
        (
            "let x = 1; function setX(v) { x = v } setX(5); return x",
            "=> 5",
        ),
        (
            "function mk() { let a = 0; return { inc: () => ++a, get: () => a } } const m = mk(); m.inc(); m.inc(); return m.get()",
            "=> 2",
        ),
    ]);
}

/// The function cases that need 0022's interpreter pieces (`special_object`,
/// `construct`, function `name`/`length`/`prototype`, `rest`). Un-ignore when 0022
/// lands. Checked against qjs too.
#[test]
fn end_to_end_functions_0022() {
    run_cases(&[
        (
            "const o = { v: 42, m: function () { return this.v } }; return o.m()",
            "=> 42",
        ),
        (
            "const o = { v: 7, m: function () { const f = () => this.v + arguments[0]; return f(100) } }; return o.m(1)",
            "=> 8",
        ),
        (
            "function foo(a, b = 1, c) {} return foo.name + foo.length",
            "=> foo1",
        ),
        (
            "const bar = () => 1; let baz; baz = function () {}; return bar.name + baz.name",
            "=> barbaz",
        ),
        ("function P(x) { this.x = x } return new P(4).x", "=> 4"),
        (
            "function F() { return new.target } return F()",
            "=> undefined",
        ),
        (
            "function F() { this.t = new.target === F } return new F().t",
            "=> true",
        ),
        (
            "function f() { return arguments.length } return f(1, 2, 3)",
            "=> 3",
        ),
        (
            "function f(a, ...r) { return a + r.length } return f(1, 2, 3)",
            "=> 3",
        ),
        ("function f(...r) { return r.length } return f()", "=> 0"),
        (
            "const f = function g(n) { return n ? g(n - 1) + 1 : 0 }; return f(3)",
            "=> 3",
        ),
    ]);
}

fn run_cases(cases: &[(&str, &str)]) {
    let mut bad = Vec::new();
    for &(src, want) in cases {
        let got = run_body(src);
        if got != want {
            bad.push(alloc::format!("{src}\n  got:  {got:?}\n  want: {want:?}"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// Exceptions end to end (docs/compiler.md 7.4). Outputs checked against qjs
/// (`work/scratch/0025/qjs_check.py end_to_end_exceptions`).
#[test]
fn end_to_end_exceptions() {
    run_cases(&[
        ("try { throw 1 } catch (e) { return e + 1 }", "=> 2"),
        ("try { throw 'x' } catch { return 'caught' }", "=> caught"),
        (
            "let log = ''; try { log += 'a' } finally { log += 'b' } return log",
            "=> ab",
        ),
        (
            "let log = ''; try { throw 1 } catch (e) { log += 'c' + e } finally { log += 'f' } return log",
            "=> c1f",
        ),
        (
            "function f() { try { return 'try' } finally { console.log('fin') } } return f()",
            "fin\n=> try",
        ),
        (
            "function f() { try { return 1 } finally { return 2 } } return f()",
            "=> 2",
        ),
        (
            "function f() { try { throw 1 } finally { return 3 } } return f()",
            "=> 3",
        ),
        (
            "let log = ''; for (let i = 0; i < 4; i++) { try { if (i == 1) continue; if (i == 3) break; log += i } finally { log += 'f' } } return log",
            "=> 0ff2ff",
        ),
        (
            "let log = ''; outer: for (let i = 0; i < 3; i++) { for (;;) { try { try { continue outer } finally { log += 'a' } } finally { log += 'b' + i } } } return log",
            "=> ab0ab1ab2",
        ),
        (
            "function f() { let log = ''; try { try { return log += 'r' } finally { log += 'x' } } finally { console.log(log) } } return f()",
            "rx\n=> r",
        ),
        (
            "let n = 0; while (true) { try { break } finally { n++ } } return n",
            "=> 1",
        ),
        (
            "let n = 0; while (true) { try { continue } finally { n++; if (n > 2) break } } return n",
            "=> 3",
        ),
        (
            "L: try { break L } finally { console.log('f') } return 'after'",
            "f\n=> after",
        ),
        (
            "try { try { throw 1 } finally { console.log('inner') } } catch (e) { return 'outer ' + e }",
            "inner\n=> outer 1",
        ),
        (
            "try { try { throw 1 } catch (e) { throw e + 1 } } catch (e) { return e }",
            "=> 2",
        ),
        (
            "try { try { throw 1 } finally { throw 2 } } catch (e) { return e }",
            "=> 2",
        ),
        (
            "function f() { throw 'deep' } function g() { return f() } try { g() } catch (e) { return e }",
            "=> deep",
        ),
        (
            "try { null.x } catch (e) { return e.message }",
            "=> cannot read property 'x' of null",
        ),
        (
            "try { undefinedName } catch (e) { return e.name + ': ' + e.message }",
            "=> ReferenceError: 'undefinedName' is not defined",
        ),
        (
            "let e = 'outer'; try { throw 'inner' } catch (e) { } return e",
            "=> outer",
        ),
        ("try { throw 1 } catch (e) { var v = e } return v", "=> 1"),
        (
            "const fs = []; for (let i = 0; i < 2; i++) { try { throw i } catch (e) { fs[i] = () => e } } return fs[0]() + fs[1]()",
            "=> 1",
        ),
        (
            "function f() { try { return 'a' } catch (e) { return 'b' } finally { } } return f()",
            "=> a",
        ),
        ("let x = 0; try { x = 1 } catch { x = 2 } return x", "=> 1"),
        (
            "switch (1) { case 1: try { break } finally { console.log('f') } } return 'ok'",
            "f\n=> ok",
        ),
        ("throw 'top'", "throw top"),
    ]);
}

/// Lexical scoping edge cases (P1-4). Outputs checked against qjs.
#[test]
fn end_to_end_lexical() {
    run_cases(&[
        (
            "typeof x; let x = 1",
            "throw ReferenceError: x is not initialized",
        ),
        (
            "let x = 1; { x; let x = 2 }",
            "throw ReferenceError: x is not initialized",
        ),
        ("let x = 1; { let x = 2; return x }", "=> 2"),
        (
            "switch (1) { case 0: let y = 1; case 1: return y }",
            "throw ReferenceError: y is not initialized",
        ),
        ("switch (0) { case 0: let y = 1; case 1: return y }", "=> 1"),
        ("const f = () => z; let z = 3; return f()", "=> 3"),
        (
            "function g() { return w } g(); let w",
            "throw ReferenceError: w is not initialized",
        ),
        (
            "let a = 1; { function a2() { return a } let a = 2; return a2() }",
            "=> 2",
        ),
        ("let x = 'outer'; { let x = 'inner' } return x", "=> outer"),
        ("const c = 1; { const c = 2 } return c", "=> 1"),
        (
            "let s = 0; for (let i = 0; i < 3; i++) { let i = 10; s += i } return s",
            "=> 30",
        ),
        (
            "let r = []; for (let i = 0; i < 3; i++) { r[i] = () => i; i += 0 } return r[0]() + r[2]()",
            "=> 2",
        ),
        ("{ let q = 1 } return typeof q", "=> undefined"),
        ("let fn; { let k = 5; fn = () => k } return fn()", "=> 5"),
    ]);
}

/// The generated prelude (D1172) compiles in privileged mode.
#[test]
fn prelude_compiles() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../builtins/gen/prelude.js");
    let src = std::fs::read(&path).unwrap();
    let mut heap = Heap::new().unwrap();
    let opts = CompileOptions::privileged_script("prelude.js");
    let t = compile(&mut heap, &src, &opts).unwrap_or_else(|e| panic!("{e}"));
    let text = disasm_template(&heap, t);
    assert!(text.contains("call_intrinsic %"), "{text}");
    // Without the flag, `%` is just an operator.
    let e = compile(&mut heap, &src, &CompileOptions::script("prelude.js")).unwrap_err();
    assert_eq!(e.kind, CompileErrorKind::Syntax);
}

/// Prelude functions that others refer to live in the realm's prelude function table
/// (D2440): `get_builtin`/`put_builtin`, no captures or Boxes.
#[test]
fn prelude_functions_use_the_table() {
    let src = "(function () {\n\
        function A(x) { return x; }\n\
        function B(x) { return A(x) + C(x); }\n\
        function C(x) { return %IsObject(x); }\n\
        function D() { return 1; }\n\
        %IsObject(D);\n\
    })();";
    let mut heap = Heap::new().unwrap();
    let opts = CompileOptions::privileged_script("p.js");
    let t = compile(&mut heap, src.as_bytes(), &opts).unwrap_or_else(|e| panic!("{e}"));
    let text = disasm_template(&heap, t);
    assert!(text.contains("put_builtin b0 2"), "{text}");
    assert!(text.contains("put_builtin b1 2"), "{text}");
    // Called with `this` undefined: `get_builtin; undefined` in one (D2443).
    assert!(text.contains("get_builtin_callee8 b0"), "{text}");
    assert!(text.contains("get_builtin_callee8 b1"), "{text}");
    for op in ["get_cap", "new_box", "put_box"] {
        assert!(!text.contains(op), "{op} in {text}");
    }
    let names = prelude_function_names(&mut heap, src.as_bytes(), &opts).unwrap();
    let names: Vec<String> = names.iter().map(|&a| heap.atom_name(a)).collect();
    assert_eq!(names, ["A", "C"]);
    // Ordinary code and functions declared deeper keep their closures.
    let nested = "(function () { function O() { function I() { return 1; } return () => I(); } %IsObject(O); })();";
    let t = compile(&mut heap, nested.as_bytes(), &opts).unwrap();
    let text = disasm_template(&heap, t);
    assert!(
        text.contains("get_cap") && !text.contains("builtin"),
        "{text}"
    );
    let e = compile(
        &mut heap,
        b"(function () { function A() {} function B() { A = 1; } %IsObject(B); })();",
        &opts,
    )
    .unwrap_err();
    assert!(
        e.message.contains("assignment to a prelude function"),
        "{}",
        e.message
    );
}

#[test]
fn privileged_errors() {
    let cases: &[(&str, &str)] = &[
        ("%Nope(1)", "unknown intrinsic %Nope"),
        ("%IsObject()", "wrong number of arguments for %IsObject"),
        ("%IsObject(1, 2)", "wrong number of arguments for %IsObject"),
        ("%Call()", "wrong number of arguments for %Call"),
        ("%IsObject(...a)", "spread arguments in an intrinsic call"),
        ("% 1", "intrinsic name expected"),
    ];
    for &(src, msg) in cases {
        let mut heap = Heap::new().unwrap();
        let e = compile(
            &mut heap,
            src.as_bytes(),
            &CompileOptions::privileged_script("p.js"),
        )
        .unwrap_err();
        assert_eq!(e.message, msg, "{src}");
    }
    let mut heap = Heap::new().unwrap();
    let ok = "%Call(f, undefined); %Call(f, undefined, 1, 2); %IsObject(1)";
    let t = compile(
        &mut heap,
        ok.as_bytes(),
        &CompileOptions::privileged_script("p.js"),
    );
    let text = disasm_template(&heap, t.unwrap());
    assert!(text.contains("call_intrinsic %Call 4"), "{text}");
    let e = compile(&mut heap, b"%IsObject(1)", &CompileOptions::script("p.js")).unwrap_err();
    assert_eq!(e.message, "unexpected token in expression: '%'");
}

/// Objects end to end (P1-2). Outputs checked against qjs
/// (`work/scratch/0027/qjs_check.py end_to_end_objects`).
#[test]
fn end_to_end_objects() {
    run_cases(&[
        (
            "const o = { a: 1, 'b c': 2, 3: 3 }; return o.a + o['b c'] + o[3]",
            "=> 6",
        ),
        (
            "const k = 'x'; const o = { [k + 'y']: 5 }; return o.xy",
            "=> 5",
        ),
        (
            "const a = 1, b = 2; const o = { a, b }; return o.a + o.b",
            "=> 3",
        ),
        ("const o = { a: 1 }; delete o.a; return o.a", "=> undefined"),
        ("const o = { a: 1 }; return delete o.a", "=> true"),
        (
            "const o = { a: 1 }; const k = 'a'; delete o[k]; return 'a' in o",
            "=> false",
        ),
        ("let o = null; return delete o?.a", "=> true"),
        (
            "let o = { a: { b: 1 } }; delete o?.a.b; return o.a.b",
            "=> undefined",
        ),
        (
            "const o = { a: 1 }; return 'a' in o && !('b' in o)",
            "=> true",
        ),
        (
            "function F() {} const f = new F(); return f instanceof F",
            "=> true",
        ),
        ("const o = { f: () => 1 }; return o.f.name", "=> f"),
    ]);
}

/// Object cases that need 0028 (method/accessor definitions, `set_proto`,
/// `set_function_name`, spread calls). Un-ignore when 0028 lands. Checked against qjs.
#[test]
fn end_to_end_objects_0028() {
    run_cases(&[
        (
            "const o = { v: 2, m(x) { return this.v * x } }; return o.m(5)",
            "=> 10",
        ),
        ("const o = { get g() { return 7 } }; return o.g", "=> 7"),
        (
            "let s = 0; const o = { set v(x) { s = x } }; o.v = 9; return s",
            "=> 9",
        ),
        (
            "const o = { _v: 1, get v() { return this._v }, set v(x) { this._v = x * 2 } }; o.v = 4; return o.v",
            "=> 8",
        ),
        (
            "const k = 'dyn'; const o = { [k]() {} }; return o.dyn.name",
            "=> dyn",
        ),
        (
            "const k = 'p'; const o = { get [k]() { return 1 } }; return o.p",
            "=> 1",
        ),
        (
            "const k = 'nm'; const o = { [k]: () => 1 }; return o.nm.name",
            "=> nm",
        ),
        ("const o = { m() {} }; return o.m.name", "=> m"),
        (
            "const p = { x: 1 }; const o = { __proto__: p }; return o.x",
            "=> 1",
        ),
        (
            "const o = { __proto__: null }; return 'toString' in o",
            "=> false",
        ),
        (
            "const o = { ...{ a: 1, b: 2 }, c: 3 }; return o.a + o.b + o.c",
            "=> 6",
        ),
        (
            "function f(a, b, c) { return a + b + c } const args = [1, 2]; return f(...args, 3)",
            "=> 6",
        ),
        (
            "const o = { m(...r) { return r.length } }; return o.m(...[1, 2], ...[3])",
            "=> 3",
        ),
        (
            "function P(a, b) { this.s = a + b } return new P(...[1, 2]).s",
            "=> 3",
        ),
    ]);
}

/// The helper of the iteration cases: `it(vals, ret)` is an iterable over `vals` that
/// logs `n` for each `next` call and `r` for each `return` call (`ret` replaces
/// `return`; `null` removes it).
const ITER_PREFIX: &str = "const log = []; function it(vals, ret) { return { [Symbol.iterator]() { let i = 0; return { next() { log.push('n'); return i < vals.length ? { value: vals[i++], done: false } : { value: undefined, done: true }; }, return: ret === undefined ? function () { log.push('r'); return {}; } : ret }; } }; }\n";

fn run_cases_with(prefix: &str, cases: &[(&str, &str)]) {
    let mut bad = Vec::new();
    for &(src, want) in cases {
        let got = run_body(&alloc::format!("{prefix}{src}"));
        if got != want {
            bad.push(alloc::format!("{src}\n  got:  {got:?}\n  want: {want:?}"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// for-of and for-in end to end (docs/compiler.md 3.8, 7.4; D1290, D1292). Outputs
/// checked against qjs (`work/scratch/0029/qjs_check.py`).
#[test]
fn end_to_end_iteration() {
    run_cases_with(
        ITER_PREFIX,
        &[
            (
                "let s = 0; for (const x of it([1, 2, 3])) s += x; return s + ' ' + log.join('')",
                "=> 6 nnnn",
            ),
            ("for (var v of it([4, 5])) ; return v", "=> 5"),
            ("let a; for (a of it([1, 2])) ; return a", "=> 2"),
            ("const o = {}; for (o.p of it([7, 8])) ; return o.p", "=> 8"),
            (
                "const o = {}; let k = 0; for (o[k++] of it(['a', 'b'])) ; return o[0] + o[1] + k",
                "=> ab2",
            ),
            (
                "for (const x of it([1, 2, 3])) { if (x == 2) break; } return log.join('')",
                "=> nnr",
            ),
            (
                "for (const x of it([1, 2, 3])) { if (x < 3) continue; console.log(x); } return log.join('')",
                "3\n=> nnnn",
            ),
            (
                "outer: for (const x of it([1, 2])) { for (const y of it([1, 2])) { continue outer; } } return log.join('')",
                "=> nnrnnrn",
            ),
            (
                "outer: for (const x of it([1, 2])) { for (const y of it([1, 2])) { break outer; } } return log.join('')",
                "=> nnrr",
            ),
            (
                "L: { for (const x of it([1, 2])) break L; } return log.join('')",
                "=> nr",
            ),
            (
                "function f() { for (const x of it([1, 2])) { return x + 10; } } return f() + ' ' + log.join('')",
                "=> 11 nr",
            ),
            (
                "function f() { for (const x of it([1])) { for (const y of it([2])) return x + y; } } return f() + ' ' + log.join('')",
                "=> 3 nnrr",
            ),
            (
                "function f() { for (const x of it([1])) { return; } } return f() + ' ' + log.join('')",
                "=> undefined nr",
            ),
            (
                "try { for (const x of it([1, 2])) throw new Error('boom'); } catch (e) { return e.message + ' ' + log.join(''); }",
                "=> boom nr",
            ),
            (
                "try { for (const x of it([1, 2], function () { throw new Error('ret'); })) throw new Error('body'); } catch (e) { return e.message; }",
                "=> body",
            ),
            (
                "try { for (const x of it([1, 2], function () { throw new Error('ret'); })) break; } catch (e) { return e.message; }",
                "=> ret",
            ),
            (
                "try { for (const x of it([1, 2], function () { return 5; })) break; } catch (e) { return String(e); }",
                "=> TypeError: not an object",
            ),
            (
                "try { for (const x of it([1, 2], function () { return 5; })) throw 'body'; } catch (e) { return String(e); }",
                "=> body",
            ),
            (
                "for (const x of it([1, 2], null)) break; return log.join('')",
                "=> n",
            ),
            (
                "function f() { for (const x of it([1, 2], function () { throw 'r'; })) return 1; } try { f(); } catch (e) { return e; }",
                "=> r",
            ),
            (
                "for (const x of it([1, 2])) { switch (x) { case 1: break; } } return log.join('')",
                "=> nnn",
            ),
            (
                "for (const x of it([1, 2])) { try { break; } finally { log.push('f'); } } return log.join('')",
                "=> nfr",
            ),
            (
                "function f() { try { for (const x of it([1])) return 'v'; } finally { log.push('f'); } } return f() + log.join('')",
                "=> vnrf",
            ),
            (
                "for (const x of it([1, 2])) { try { throw 1; } catch (e) { log.push('c'); } } return log.join('')",
                "=> ncncn",
            ),
            (
                "try { for (const x of it([1, 2], function () { log.push('r'); throw 'r'; })) { try { break; } finally { log.push('f'); } } } catch (e) { log.push('c' + e); } return log.join('')",
                "=> nfrcr",
            ),
            (
                "const o = {}; Object.defineProperty(o, 'p', { set(v) { if (v == 2) throw 'set'; } }); try { for (o.p of it([1, 2, 3])) ; } catch (e) { return e + log.join(''); }",
                "=> setnnr",
            ),
            (
                "try { for (const x of { [Symbol.iterator]() { return { next() { return 1; }, return() { log.push('r'); } }; } }) ; } catch (e) { return String(e) + log.join(''); }",
                "=> TypeError: iterator must return an object",
            ),
            (
                "try { for (const x of { [Symbol.iterator]() { return { next() { throw 'next'; }, return() { log.push('r'); } }; } }) ; } catch (e) { return e + log.join(''); }",
                "=> next",
            ),
            (
                "try { for (const x of { [Symbol.iterator]() { return { next() { return { get done() { throw 'done'; } }; }, return() { log.push('r'); } }; } }) ; } catch (e) { return e + log.join(''); }",
                "=> done",
            ),
            (
                "try { for (const x of { [Symbol.iterator]() { return { next() { return { done: false, get value() { throw 'value'; } }; }, return() { log.push('r'); } }; } }) ; } catch (e) { return e + log.join(''); }",
                "=> value",
            ),
            (
                "try { for (const x of 1) ; } catch (e) { return String(e); }",
                "=> TypeError: value is not iterable",
            ),
            (
                "try { for (const x of {}) ; } catch (e) { return String(e); }",
                "=> TypeError: value is not iterable",
            ),
            (
                "try { for (const x of { [Symbol.iterator]: 5 }) ; } catch (e) { return String(e); }",
                "=> TypeError: value is not iterable",
            ),
            (
                "try { for (const x of { [Symbol.iterator]() { return 1; } }) ; } catch (e) { return String(e); }",
                "=> TypeError: not an object",
            ),
            (
                "try { for (const x of { [Symbol.iterator]() { return {}; } }) ; } catch (e) { return String(e); }",
                "=> TypeError: not a function",
            ),
            (
                "let n = 0; const iter = { [Symbol.iterator]() { return this; }, next() { n++; this.next = null; return { done: n > 2, value: n }; } }; let s = ''; for (const x of iter) s += x; return s",
                "=> 12",
            ),
            (
                "const fs = []; for (const x of it([1, 2, 3])) fs.push(() => x); return fs.map((f) => f()).join()",
                "=> 1,2,3",
            ),
            (
                "const fs = []; for (let x of it([1, 2, 3])) { fs.push(() => x); x *= 10; } return fs.map((f) => f()).join()",
                "=> 10,20,30",
            ),
            (
                "const fs = []; for (var x of it([1, 2, 3])) fs.push(() => x); return fs.map((f) => f()).join()",
                "=> 3,3,3",
            ),
            (
                "try { for (let x of x) ; } catch (e) { return String(e); }",
                "=> ReferenceError: x is not initialized",
            ),
            (
                "try { for (const x of it([1])) x = 2; } catch (e) { return String(e) + log.join(''); }",
                "=> TypeError: 'x' is read-onlynr",
            ),
            (
                "let x = 'outer'; for (let x of it(['inner'])) { } return x",
                "=> outer",
            ),
            (
                "let r = ''; for (const x of it(['a'])) { let x2 = x + 'b'; { let x = 'c'; r += x2 + x; } } return r",
                "=> abc",
            ),
            (
                "const ks = []; const o = { b: 1, a: 2, 2: 3, 1: 4 }; for (const k in o) ks.push(k); return ks.join()",
                "=> 1,2,b,a",
            ),
            (
                "const ks = []; const p = { x: 1, y: 2 }; const o = Object.create(p); o.z = 3; o.x = 4; for (const k in o) ks.push(k); return ks.join()",
                "=> z,x,y",
            ),
            (
                "const ks = []; const o = { a: 1 }; Object.defineProperty(o, 'h', { value: 1, enumerable: false }); for (const k in o) ks.push(k); return ks.join()",
                "=> a",
            ),
            (
                "const ks = []; const p = { s: 1 }; const o = Object.create(p); Object.defineProperty(o, 's', { value: 1, enumerable: false }); for (const k in o) ks.push(k); return ks.length",
                "=> 0",
            ),
            (
                "const ks = []; const o = { a: 1, b: 2, c: 3 }; for (const k in o) { ks.push(k); delete o.b; o.d = 4; } return ks.join()",
                "=> a,c",
            ),
            (
                "const ks = []; const o = { [Symbol('s')]: 1, a: 1 }; for (const k in o) ks.push(typeof k); return ks.join()",
                "=> string",
            ),
            (
                "let n = 0; for (const k in null) n++; for (const k in undefined) n++; return n",
                "=> 0",
            ),
            (
                "const ks = []; for (const k in 'ab') ks.push(k); return ks.join()",
                "=> 0,1",
            ),
            (
                "const ks = []; for (const k in 5) ks.push(k); return ks.length",
                "=> 0",
            ),
            (
                "const ks = []; for (const k in [7, 8]) ks.push(typeof k + k); return ks.join()",
                "=> string0,string1",
            ),
            (
                "const o = {}; for (o.k in { a: 1, b: 2 }) ; return o.k",
                "=> b",
            ),
            ("var k; for (k in { a: 1 }) ; return k", "=> a"),
            (
                "let s = ''; for (const k in { a: 1, b: 2, c: 3 }) { if (k == 'b') continue; if (k == 'c') break; s += k; } return s",
                "=> a",
            ),
            (
                "let s = ''; outer: for (const k in { a: 1, b: 1 }) { for (const j in { x: 1, y: 1 }) { s += k + j; continue outer; } } return s",
                "=> axbx",
            ),
            (
                "function f() { for (const k in { a: 1 }) { for (const x of it([1])) return k + x; } } return f() + log.join('')",
                "=> a1nr",
            ),
            (
                "const fs = []; for (const k in { a: 1, b: 2 }) fs.push(() => k); return fs.map((f) => f()).join()",
                "=> a,b",
            ),
            (
                "try { for (let k in k) ; } catch (e) { return String(e); }",
                "=> ReferenceError: k is not initialized",
            ),
            (
                "let s = ''; for (const k in { a: 1 }) for (const j in { b: 1 }) s += k + j; return s",
                "=> ab",
            ),
            (
                "let i = 0; for (const x of it([1, 2, 3])) { i++; } return i + log.join('')",
                "=> 3nnnn",
            ),
        ],
    );
}

/// Iteration over arrays (the Array Iterator leaves of 0030, D1290). Outputs checked
/// against qjs.
#[test]
fn end_to_end_iteration_rt() {
    run_cases(&[
        (
            "let s = 0; for (const x of [1, 2, 3]) s += x; return s",
            "=> 6",
        ),
        (
            "let s = ''; for (const x of [1, , 3]) s += x; return s",
            "=> 1undefined3",
        ),
        (
            "const a = [1, 2]; let s = ''; for (const x of a) { s += x; if (a.length < 4) a.push(a.length + 1); } return s",
            "=> 1234",
        ),
        (
            "const a = [1, 2, 3]; let s = ''; for (const x of a) { s += x; a.length = 1; } return s",
            "=> 1",
        ),
        (
            "let s = ''; for (const e of ['a', 'b'].entries()) s += e[0] + e[1]; return s",
            "=> 0a1b",
        ),
        (
            "let s = ''; for (const k of ['a', 'b'].keys()) s += k; return s",
            "=> 01",
        ),
        (
            "let s = ''; for (const v of ['a', 'b'].values()) s += v; return s",
            "=> ab",
        ),
        (
            "const it = [1, 2][Symbol.iterator](); return it[Symbol.iterator]() === it",
            "=> true",
        ),
        (
            "const it = [1][Symbol.iterator](); const r1 = it.next(); const r2 = it.next(); return r1.value + ' ' + r1.done + ' ' + r2.value + ' ' + r2.done",
            "=> 1 false undefined true",
        ),
        (
            "try { [].values().next.call({}); } catch (e) { return String(e); }",
            "=> TypeError: Array Iterator object expected",
        ),
        (
            "let s = ''; for (const x of { length: 2, 0: 'a', 1: 'b', [Symbol.iterator]: Array.prototype.values }) s += x; return s",
            "=> ab",
        ),
        (
            "const p = Array.prototype[Symbol.iterator]; let n = 0; Array.prototype[Symbol.iterator] = function () { n++; return p.call(this); }; for (const x of [1]) ; Array.prototype[Symbol.iterator] = p; return n",
            "=> 1",
        ),
        (
            "let s = ''; for (const x of [1, 2, 3]) { if (x == 2) break; s += x; } return s",
            "=> 1",
        ),
        (
            "const fs = []; for (const x of [1, 2]) fs.push(() => x); return fs[0]() + fs[1]()",
            "=> 3",
        ),
        (
            "return Object.getPrototypeOf(Object.getPrototypeOf([].values())) === Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()))",
            "=> true",
        ),
        ("return [].values === [][Symbol.iterator]", "=> true"),
        (
            "const d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf([].values()), Symbol.toStringTag); return Object.prototype.toString.call([].values()) + ' ' + d.value + d.writable + d.enumerable + d.configurable",
            "=> [object Array Iterator] Array Iteratorfalsefalsetrue",
        ),
    ]);
}

/// Destructuring and tagged templates end to end (compiler/pattern.rs, D1360-D1362).
/// Outputs checked against qjs (`work/scratch/0036/qjs_check.py`).
#[test]
fn end_to_end_destructuring() {
    run_cases(&[
        (
            "let [a, b = 2, , ...c] = [1, undefined, 3, 4, 5]; return a + ' ' + b + ' ' + c.join()",
            "=> 1 2 4,5",
        ),
        (
            "const {x, y: {z = 9} = {}, ...r} = {x: 1, q: 2, w: 3}; return x + ' ' + z + ' ' + Object.keys(r).join()",
            "=> 1 9 q,w",
        ),
        ("var [p, q] = 'hi'; return p + q", "=> hi"),
        (
            "let m = 1, n = 2; [m, n] = [n, m]; return m + ' ' + n",
            "=> 2 1",
        ),
        (
            "const o = {}; ({a: o.x, b: o['y'] = 5} = {a: 7}); return o.x + ' ' + o.y",
            "=> 7 5",
        ),
        (
            "try { throw [1, {k: 2}] } catch ([e1, {k}]) { return e1 + k }",
            "=> 3",
        ),
        (
            "let [a, [b, [c]]] = [1, [2, [3]]]; return a + b + c",
            "=> 6",
        ),
        (
            "let {a: {b: {c}}} = {a: {b: {c: 'deep'}}}; return c",
            "=> deep",
        ),
        (
            "let x; const r = ({x} = {x: 3}); return x + ' ' + r.x",
            "=> 3 3",
        ),
        (
            "let a, b; const r = [a, b] = [1, 2, 3]; return r.length",
            "=> 3",
        ),
        (
            "const [a = 1, b = a + 1, c = b * 2] = []; return [a, b, c].join()",
            "=> 1,2,4",
        ),
        ("const {length} = 'abc'; return length", "=> 3"),
        ("const [, , third] = [1, 2, 3]; return third", "=> 3"),
        ("const [...all] = 'ab'; return all.join('-')", "=> a-b"),
        ("const [a, ...[b, c]] = [1, 2, 3]; return a + b + c", "=> 6"),
        ("const {0: a, 1: b} = ['x', 'y']; return a + b", "=> xy"),
        (
            "const k = 'dyn'; const {[k]: v, [k + '2']: w = 'def'} = {dyn: 1}; return v + w",
            "=> 1def",
        ),
        (
            "const {a, ...rest} = {a: 1, b: 2, c: 3}; return a + ' ' + Object.keys(rest).join() + rest.b",
            "=> 1 b,c2",
        ),
        (
            "const s = Symbol('s'); const {...r} = {[s]: 1, x: 2}; return r[s] + r.x",
            "=> 3",
        ),
        (
            "const {['a']: x, ...r} = {a: 1, b: 2}; return Object.keys(r).join()",
            "=> b",
        ),
        (
            "const {0: x, ...r} = ['a', 'b']; return Object.keys(r).join()",
            "=> 1",
        ),
        (
            "const {...r} = Object.create({inherited: 1}, {own: {value: 2, enumerable: true}, hidden: {value: 3}}); return Object.keys(r).join()",
            "=> own",
        ),
        (
            "let n = 0; const {a = n++, b = n++} = {a: 0}; return a + ' ' + b + ' ' + n",
            "=> 0 0 1",
        ),
        (
            "const [f = function () {}, g = () => 1] = []; return f.name + g.name",
            "=> fg",
        ),
        ("const {fn = function () {}} = {}; return fn.name", "=> fn"),
        ("let fn; [fn = () => 1] = []; return fn.name", "=> fn"),
        ("let fn; ({fn = () => 1} = {}); return fn.name", "=> fn"),
        (
            "try { const {a} = null; } catch (e) { return String(e) }",
            "=> TypeError: cannot convert to object",
        ),
        (
            "try { const {} = undefined; } catch (e) { return String(e) }",
            "=> TypeError: cannot convert to object",
        ),
        (
            "try { let [a] = 1; } catch (e) { return String(e) }",
            "=> TypeError: value is not iterable",
        ),
        (
            "try { let [a] = {}; } catch (e) { return String(e) }",
            "=> TypeError: value is not iterable",
        ),
        (
            "try { let {a = b, b} = {}; } catch (e) { return e.name }",
            "=> ReferenceError",
        ),
        (
            "try { let [a = a] = []; } catch (e) { return e.name }",
            "=> ReferenceError",
        ),
        (
            "function f({a, b = 2}, [c, ...d] = [3, 4, 5], ...[e, g]) { return [a, b, c, d.length, e, g].join(); } return f({a: 1}) + ' ' + f({a: 1, b: 0}, [7], 8, 9) + ' ' + f.length",
            "=> 1,2,3,2,, 1,0,7,0,8,9 1",
        ),
        (
            "const h = ({x}, [y]) => x + y; return h({x: 1}, [2]) + ' ' + h.length",
            "=> 3 2",
        ),
        (
            "function f([a] = [1], {b} = {b: 2}) { return a + b } return f() + ' ' + f.length",
            "=> 3 0",
        ),
        (
            "function f(a, {b}) { return a + b } return f(1, {b: 2}) + f.length",
            "=> 5",
        ),
        (
            "function f({a}, b = a) { return b } return f({a: 5})",
            "=> 5",
        ),
        (
            "try { (function ([a = b, b]) {})([]) } catch (e) { return e.name }",
            "=> ReferenceError",
        ),
        (
            "function f({a}) { return arguments.length } return f({}, 2)",
            "=> 2",
        ),
        (
            "function f({a}) { return () => a } return f({a: 'cap'})()",
            "=> cap",
        ),
        ("function f([x]) { x = 2; return x } return f([1])", "=> 2"),
        (
            "let out = ''; for (const [k, v] of Object.entries({p: 1, q: 2})) out += k + v; return out",
            "=> p1q2",
        ),
        (
            "let out = ''; for (var {length} in {abc: 1, de: 2}) out += length; return out",
            "=> 32",
        ),
        (
            "let a, b, out = ''; for ([a, b] of [[1, 2], [3, 4]]) out += a + b; return out",
            "=> 37",
        ),
        ("let a; for ({a} of [{a: 5}]) ; return a", "=> 5"),
        (
            "const fs = []; for (let [i] of [[1], [2]]) fs.push(() => i); return fs.map(f => f()).join()",
            "=> 1,2",
        ),
        (
            "const fs = []; for (const {i} of [{i: 1}, {i: 2}]) fs.push(() => i); return fs[0]() + fs[1]()",
            "=> 3",
        ),
        (
            "const o = {}; for ([o.a, o.b] of [[1, 2]]) ; return o.a + o.b",
            "=> 3",
        ),
        (
            "let x = 0; for (const [a = ++x] of [[], [5]]) ; return x",
            "=> 1",
        ),
        (
            "try { throw {code: 42} } catch ({code, missing = 'm'}) { return code + missing }",
            "=> 42m",
        ),
        (
            "try { throw null } catch ({a}) { return 1 } finally {}",
            "throw TypeError: cannot convert to object",
        ),
        (
            "const o = {a: 1}; let a; ({a, ...o.rest} = {a: 1, b: 2}); return Object.keys(o).join() + o.rest.b",
            "=> a,rest2",
        ),
        ("let x; [x = 1] = [null]; return x", "=> null"),
        ("let x; ({x = 1} = {x: null}); return x", "=> null"),
        ("const {a: {b} = {b: 'dflt'}} = {}; return b", "=> dflt"),
        ("const [[a] = [7]] = []; return a", "=> 7"),
        ("const {a: x = 1, b: y = 2} = {b: 3}; return x + y", "=> 4"),
        ("let a = 1; [a, a] = [2, 3]; return a", "=> 3"),
        (
            "const o = {}; ({__proto__: o.a, __proto__: o.b} = {}); return o.a === o.b",
            "=> true",
        ),
        (
            "let r; [...r] = []; return Array.isArray(r) + ' ' + r.length",
            "=> true 0",
        ),
        ("let [] = []; const {} = {}; return 'ok'", "=> ok"),
        ("const [a, b] = 'x'; return a + b", "=> xundefined"),
        ("return (([a, b] = [1, 2]) => a + b)()", "=> 3"),
        ("return (({a = 1} = {}) => a)()", "=> 1"),
        (
            "let s = ''; const o = {get a() { s += 'a'; return 1 }, get b() { s += 'b'; return 2 }}; const {b, a} = o; return s",
            "=> ba",
        ),
        (
            "const {length: len, 0: first} = 'abc'; return len + first",
            "=> 3a",
        ),
        (
            "function tag(s, ...v) { return s.join('|') + '/' + s.raw.join('|') + '/' + v.join(); } return tag`a${1}b${2}c`",
            "=> a|b|c/a|b|c/1,2",
        ),
        (
            "function tag(s) { return s[0] + ' ' + s.raw[0] } return tag`\\unicode`",
            "=> undefined \\unicode",
        ),
        (
            "function id(s) { return s } const a = []; for (let i = 0; i < 2; i++) a.push(id`x`); return (a[0] === a[1]) + ' ' + (id`x` === a[0])",
            "=> true false",
        ),
        (
            "function id(s) { return s } const s = id`a\\nb${0}`; return Object.isFrozen(s) + ' ' + Object.isFrozen(s.raw) + ' ' + s.length + ' ' + s.raw[0] + ' ' + Object.getOwnPropertyDescriptor(s, 'raw').enumerable",
            "=> true true 2 a\\nb false",
        ),
        (
            "const o = {m(s) { return this === o }}; return o.m`x` && o['m']`y`",
            "=> true",
        ),
        (
            "return (s => s[0] === undefined && s.raw[0] === '\\\\xg')`\\xg`",
            "=> true",
        ),
        (
            "function tag(s, a, b) { return s.length + ' ' + a + b } return tag`${1}${2}`",
            "=> 3 12",
        ),
        (
            "function f() { return f } return f`a``b`` c` === f",
            "=> true",
        ),
        (
            "const t = (s) => s; return t`\r\n`.raw[0] === '\\n'",
            "=> true",
        ),
        (
            "const o = {...{a: 1, get b() { return 2 }}}; return Object.keys(o).join() + o.b",
            "=> a,b2",
        ),
        // qjs copies nothing from a string; the spec (and V8) copy its indexes.
        (
            "const o = {...'hi'}; return Object.keys(o).join()",
            "=> 0,1",
        ),
        (
            "const o = {...null, ...undefined, ...1, ...[5]}; return Object.keys(o).join() + o[0]",
            "=> 05",
        ),
    ]);
}

/// Array destructuring and the iterator protocol: steps, closing on normal and abrupt
/// completion, and evaluation order (D1361). Outputs checked against qjs
/// (`work/scratch/0036/qjs_check.py`).
#[test]
fn end_to_end_destructuring_iteration() {
    run_cases_with(
        ITER_PREFIX,
        &[
            ("const [a] = it([1, 2]); return log.join('')", "=> nr"),
            ("const [a, b] = it([1]); return log.join('')", "=> nn"),
            (
                "const [...a] = it([1, 2]); return a.join() + log.join('')",
                "=> 1,2nnn",
            ),
            ("const [] = it([1]); return log.join('')", "=> r"),
            ("const [, ] = it([1, 2]); return log.join('')", "=> nr"),
            (
                "const [a, b, c] = it([1]); return [a, b, c].join() + log.join('')",
                "=> 1,,nn",
            ),
            (
                "const [a, b, ...c] = it([1]); return c.length + log.join('')",
                "=> 0nn",
            ),
            (
                "try { const [a = (() => { throw 1 })()] = it([undefined]) } catch (e) { return e + log.join('') }",
                "=> 1nr",
            ),
            (
                "const o = { set x(v) { throw 'set' } }; try { [o.x] = it([1]) } catch (e) { return e + log.join('') }",
                "=> setnr",
            ),
            (
                "const bad = { [Symbol.iterator]() { return { next() { throw 'nx' }, return() { log.push('r') } } } }; try { const [a] = bad } catch (e) { return e + log.join('') }",
                "=> nx",
            ),
            (
                "const bad = { [Symbol.iterator]() { return { next() { return 1 }, return() { log.push('r') } } } }; try { const [a] = bad } catch (e) { return String(e) + log.join('') }",
                "=> TypeError: iterator must return an object",
            ),
            (
                "const bad = { [Symbol.iterator]() { return { next() { return { get done() { throw 'dn' } } }, return() { log.push('r') } } } }; try { const [a] = bad } catch (e) { return e + log.join('') }",
                "=> dn",
            ),
            (
                "try { const [a] = it([1, 2], function () { throw 'rt' }) } catch (e) { return e + log.join('') }",
                "=> rtn",
            ),
            (
                "try { const [a] = it([1, 2], function () { return 1 }) } catch (e) { return String(e) + log.join('') }",
                "=> TypeError: not an objectn",
            ),
            (
                "try { [{}[(() => { throw 'k' })()]] = it([1], function () { throw 'rt' }) } catch (e) { return e + log.join('') }",
                "=> k",
            ),
            (
                "const [[a]] = [it([1, 2])]; return a + log.join('')",
                "=> 1nr",
            ),
            (
                "try { const [[a] = (() => { throw 'inner' })()] = it([undefined, 1]) } catch (e) { return e + log.join('') }",
                "=> innernr",
            ),
            (
                "try { const [{x}] = it([null, 2]) } catch (e) { return e.name + log.join('') }",
                "=> TypeErrornr",
            ),
            (
                "try { const [a, [b]] = it([1, 5]) } catch (e) { return e.name + log.join('') }",
                "=> TypeErrornnr",
            ),
            (
                "const order = []; const o = { get t() { order.push('get t'); return { set x(v) { order.push('set ' + v) } } } }; [o.t.x] = { [Symbol.iterator]() { order.push('iter'); return { next() { order.push('next'); return { done: false, value: 1 } }, return() { order.push('ret'); return {} } } } }; return order.join()",
                "=> iter,get t,next,set 1,ret",
            ),
            (
                "const order = []; const src = { get a() { order.push('get a'); return 1 } }; const key = { toString() { order.push('key'); return 'a' } }; const t = { set x(v) { order.push('set ' + v) } }; ({ [key]: (order.push('target'), t).x } = src); return order.join()",
                "=> key,target,get a,set 1",
            ),
            (
                "const order = []; const src = { get a() { order.push('get a'); return undefined } }; const t = { set x(v) { order.push('set ' + v) } }; ({ a: t.x = (order.push('default'), 2) } = src); return order.join()",
                "=> get a,default,set 2",
            ),
            (
                "const order = []; const k = { toString() { order.push('key'); return 'b' } }; const { [k]: v, ...r } = { a: 1, b: 2, get c() { order.push('get c'); return 3 } }; return order.join() + ' ' + Object.keys(r).join()",
                "=> key,get c a,c",
            ),
            (
                "for (const [a] of [it([1, 2]), it([3])]) ; return log.join('')",
                "=> nrnr",
            ),
            (
                "function f([a, b]) { return a } f(it([1, 2, 3])); return log.join('')",
                "=> nnr",
            ),
            (
                "try { throw it([1, 2]) } catch ([a]) { return a + log.join('') }",
                "=> 1nr",
            ),
            (
                "let a; for ([a] of [it([1, 2])]) ; return a + log.join('')",
                "=> 1nr",
            ),
            (
                "label: for (const x of [1]) { const [a] = it([x, 2]); break label; } return log.join('')",
                "=> nr",
            ),
            (
                "let a; const r = ([a] = it([1, 2])); return (a === 1) + log.join('')",
                "=> truenr",
            ),
        ],
    );
}

/// Messages from %GetIterator on nullish values (a symbol key in "cannot read
/// property", as qjs names it).
#[test]
fn end_to_end_iteration_messages() {
    run_cases(&[(
        "try { for (const x of undefined) ; } catch (e) { return String(e); }",
        "=> TypeError: cannot read property 'Symbol.iterator' of undefined",
    )]);
}

/// Async functions, generators and async generators end to end (0041), in async-body
/// mode: the body's promise gives the result. Checked against qjs.
#[test]
fn end_to_end_async_generators() {
    run_cases(&[
        (
            "async function f() { return 1 } return await f() + 1",
            "=> 2",
        ),
        (
            "const p = Promise.resolve(3); return (await p) * (await 2)",
            "=> 6",
        ),
        ("throw new TypeError('x')", "throw TypeError: x"),
        (
            "async function f() { await null; throw 'late' } try { await f() } catch (e) { return 'caught ' + e }",
            "=> caught late",
        ),
        (
            "const log = []; const f = async () => { log.push(1); await 0; log.push(3) }; const p = f(); log.push(2); await p; return log.join()",
            "=> 1,2,3",
        ),
        (
            "function* g() { yield 1; yield 2 } return [...g()].join()",
            "=> 1,2",
        ),
        (
            "function* g() { try { yield 1 } finally { console.log('fin') } } const it = g(); it.next(); return it.return(5).value",
            "fin\n=> 5",
        ),
        (
            "function* g() { const x = yield 1; return x * 2 } const it = g(); it.next(); return it.next(21).value",
            "=> 42",
        ),
        (
            "function* g() { return yield* [1, 2] } const it = g(); it.next(); it.next(); return it.next().done",
            "=> true",
        ),
        (
            "let s = 0; for await (const x of [1, Promise.resolve(2)]) s += x; return s",
            "=> 3",
        ),
        (
            "async function* a() { yield 1; yield await Promise.resolve(2) } const r = []; for await (const x of a()) r.push(x); return r.join()",
            "=> 1,2",
        ),
        (
            "const o = { async m() { return this.v }, *n() { yield this.v }, v: 7 }; return (await o.m()) + [...o.n()][0]",
            "=> 14",
        ),
    ]);
}

/// Script and eval code evaluate to their completion value (0058; checked with qjs's
/// `eval`).
#[test]
fn completion_values() {
    let cases: &[(&str, &str)] = &[
        ("", "=> undefined"),
        ("1", "=> 1"),
        ("1; var x = 2;", "=> 1"),
        ("1; let y = 2; function f() { 3 }", "=> 1"),
        ("1; ;", "=> 1"),
        ("1; {}", "=> 1"),
        ("1; { 2; }", "=> 2"),
        ("1; if (true) {}", "=> undefined"),
        ("1; if (true) { 2 }", "=> 2"),
        ("1; if (false) 2;", "=> undefined"),
        ("1; if (false) 2; else 3;", "=> 3"),
        ("1; while (false);", "=> undefined"),
        ("var i = 0; while (i < 3) i++;", "=> 2"),
        ("1; do { 2; break; } while (true)", "=> 2"),
        ("1; for (var j = 0; j < 2; j++) { j; }", "=> 1"),
        ("1; for (const k of [5, 6]) k * 2;", "=> 12"),
        ("1; for (const k in {}) k;", "=> undefined"),
        ("1; switch (1) {}", "=> undefined"),
        ("1; switch (1) { case 1: 2; case 3: 4; }", "=> 4"),
        ("1; switch (1) { case 1: 2; break; case 3: 4; }", "=> 2"),
        ("1; try { 2 } catch (e) {}", "=> 2"),
        ("1; try { 2; throw 0 } catch (e) {}", "=> undefined"),
        ("1; try { 2; throw 0 } catch (e) { 3 }", "=> 3"),
        ("1; try { 2 } finally { 3 }", "=> 2"),
        ("1; try {} finally { 3 }", "=> undefined"),
        ("1; L: try { 2 } finally { 3; break L; }", "=> 3"),
        ("1; L: { 2; break L; }", "=> 2"),
        ("1; class C {}", "=> 1"),
        ("1; (function () { 5; })();", "=> undefined"),
        ("var x = 1; x += 1;", "=> 2"),
        ("1; { using u = null; 4; }", "=> 4"),
    ];
    for goal in [CompileOptions::script("t.js"), CompileOptions::eval("t.js")] {
        let mut bad = Vec::new();
        for &(src, want) in cases {
            let got = run_opts(src, &goal);
            if got != want {
                bad.push(alloc::format!("{src:?}: got {got:?}, want {want:?}"));
            }
        }
        assert!(bad.is_empty(), "{:?}:\n{}", goal.goal, bad.join("\n"));
    }
}

/// Eval code (0058): top-level declarations are locals of the call, `this` is what the
/// call gets, and the function-only syntax is rejected.
#[test]
fn eval_goal() {
    let eval = CompileOptions::eval("t.js");
    let cases: &[(&str, &str)] = &[
        (
            "var a = 1; let b = 2; const c = 3; function d() {} typeof globalThis.a",
            "=> undefined",
        ),
        ("var a = 1; a", "=> 1"),
        ("this === globalThis", "=> true"),
        ("typeof undefinedName", "=> undefined"),
        ("Math.max(1, 2)", "=> 2"),
        ("var await = 1; await", "=> 1"),
        (
            "return 1",
            "compile error: SyntaxError: return not in a function at 1:1",
        ),
        (
            "new.target",
            "compile error: SyntaxError: new.target only allowed within functions at 1:1",
        ),
        (
            "super.x",
            "compile error: SyntaxError: 'super' is only valid in a method at 1:6",
        ),
        (
            "using x = null;",
            "compile error: SyntaxError: using declaration not allowed here at 1:7",
        ),
    ];
    let mut bad = Vec::new();
    for &(src, want) in cases {
        let got = run_opts(src, &eval);
        if got != want {
            bad.push(alloc::format!("{src:?}: got {got:?}, want {want:?}"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// The opcodes of function `f` in `src` (a Script), in order (0086 peepholes).
fn ops_of_f(src: &str) -> Vec<String> {
    let mut heap = Heap::new().unwrap();
    let t = compile_src(&mut heap, src).unwrap();
    let text = disasm_template(&heap, t);
    let body = text.split("function 0.0 f:").nth(1).expect("function f");
    let body = body.split("\nfunction ").next().unwrap_or(body);
    body.lines()
        .skip(1)
        .filter(|l| !l.trim_start().starts_with("handler"))
        .filter_map(|l| l.split_whitespace().nth(1))
        // The wide names: short forms are an encoding (D2443), not a peephole.
        .map(
            |n| match crate::bytecode::Op::ALL.iter().find(|o| o.name() == n) {
                Some(o) => String::from(o.wide().name()),
                None => String::from(n),
            },
        )
        .collect()
}

/// Array patterns in unreachable code (0210): their close handlers' relative depths
/// have no anchor to come from, so they (and the handlers only dead code reaches) are
/// dropped, while the live pattern beside them keeps its handler.
#[test]
fn dead_pattern_handlers_dropped() {
    let src = "function f() { let [a = 1] = []; return a; let [b = 1] = []; }\n\
               function* g() { return 1; let [a = yield* h()] = []; }\n\
               function k() { throw 1; try { let [a = 1] = []; } catch { let [b = 1] = []; } }";
    let mut heap = Heap::new().unwrap();
    let t = compile_src(&mut heap, src).unwrap();
    let text = disasm_template(&heap, t);
    let handlers = |name: &str| {
        let body = text.split(&alloc::format!(" {name}: ")).nth(1).unwrap();
        let body = body.split("\nfunction ").next().unwrap();
        body.lines()
            .filter(|l| l.trim_start().starts_with("handler"))
            .count()
    };
    assert_eq!(handlers("f"), 1, "{text}");
    assert_eq!(handlers("g"), 0, "{text}");
    assert_eq!(handlers("k"), 0, "{text}");
    run_cases(&[
        (
            "function f() { let [a = 1] = []; return a; let [b = 1] = []; } return f()",
            "=> 1",
        ),
        ("return 2; let [a = 1] = [];", "=> 2"),
        (
            "function* g() { return 3; let [a = yield* g()] = []; } return g().next().value",
            "=> 3",
        ),
    ]);
}

#[test]
fn peepholes() {
    let cases: &[(&str, &[&str])] = &[
        // Assignment statements store without keeping the value.
        (
            "function f(o, v) { let x; x = v; o.c = v; o[v] = 1; }",
            &[
                "undefined",
                "put_loc",
                "get_arg",
                "put_loc",
                "get_arg",
                "get_arg",
                "put_field",
                "get_arg",
                "get_arg",
                "push_i8",
                "put_elem",
                "return_undef",
            ],
        ),
        // `i++;`, `--i;` on a local; `o.x++;` without the copy of the old value.
        (
            "function f(o) { let i = 0; i++; --i; o.x++; }",
            &[
                "push_i8",
                "put_loc",
                "inc_loc",
                "dec_loc",
                "get_arg",
                "dup",
                "get_field",
                "inc",
                "put_field",
                "return_undef",
            ],
        ),
        // A rotated loop: one conditional jump per iteration.
        (
            "function f(n) { let s = 0; for (let i = 0; i < n; i++) s += i; return s; }",
            &[
                "push_i8",
                "put_loc",
                "push_i8",
                "put_loc",
                "jump",
                "get_loc",
                "get_loc",
                "add",
                "put_loc",
                "inc_loc",
                "get_loc",
                "get_arg",
                "lt",
                "jump_if_true",
                "get_loc",
                "return",
            ],
        ),
        (
            "function f(n) { while (n) n--; }",
            &[
                "jump",
                "get_arg",
                "dec",
                "put_arg",
                "get_arg",
                "jump_if_true",
                "return_undef",
            ],
        ),
        // `while (true)` and `if (c) break;`
        (
            "function f(c) { while (true) { if (c) break; } }",
            &["get_arg", "jump_if_true", "jump", "return_undef"],
        ),
        // `!`, `||` in conditions, `=== undefined`, a jump to a return.
        (
            "function f(a, b) { if (!a || b === undefined) return 1; return a ? 2 : 3; }",
            &[
                "get_arg",
                "jump_if_false",
                "get_arg",
                "is_undefined",
                "jump_if_false",
                "push_i8",
                "return",
                "get_arg",
                "jump_if_false",
                "push_i8",
                "return",
                "push_i8",
                "return",
            ],
        ),
        // A stored value used right away.
        (
            "function f(g) { const p = g(); return p; }",
            &["get_arg", "undefined", "call", "set_loc", "return"],
        ),
    ];
    let mut bad = Vec::new();
    for &(src, want) in cases {
        let got = ops_of_f(src);
        if got != want {
            bad.push(alloc::format!("{src}\n  got:  {got:?}\n  want: {want:?}"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// Module code's early errors (16.2.1.1) and what it rejects.
#[test]
fn module_errors() {
    let cases: &[(&str, &str)] = &[
        ("export { x }", "exported variable 'x' does not exist"),
        ("let x; export { x, x }", "duplicate exported name 'x'"),
        (
            "export default 1; export default 2",
            "invalid redefinition of lexical identifier",
        ),
        (
            "let a, b; export { a as c, b as c }",
            "duplicate exported name 'c'",
        ),
        (
            "export * as a from 'm'; export let a",
            "duplicate exported name 'a'",
        ),
        (
            "import { a } from 'm'; let a",
            "invalid redefinition of lexical identifier",
        ),
        (
            "import a from 'm'; var a",
            "invalid redefinition of lexical identifier",
        ),
        (
            "function f() {} function f() {}",
            "invalid redefinition of lexical identifier",
        ),
        (
            "var f; function f() {}",
            "invalid redefinition of lexical identifier",
        ),
        (
            "function f() {} var f",
            "invalid redefinition of lexical identifier",
        ),
        ("var await", "variable name expected"),
        ("function f() { var await }", "variable name expected"),
        ("import { await } from 'm'", "variable name expected"),
        ("import { default } from 'm'", "variable name expected"),
        ("export { default }", "identifier expected"),
        ("export { 'a' }", "identifier expected"),
        ("return 1", "return not in a function"),
        ("new.target", "new.target only allowed within functions"),
        ("{ export let x }", "unsupported keyword: export"),
        ("function f() { import 'm' }", "unsupported keyword: import"),
        ("export 1", "unexpected token"),
        (
            "import a from 'm' with { type: 'json', type: 'json' }",
            "duplicate import attribute",
        ),
        ("import a from 'm' with { type: 1 }", "expecting string"),
        (
            "export { '\\uD800' as a } from 'm'",
            "invalid module export name",
        ),
        ("import * from 'm'", "expecting 'as'"),
    ];
    for &(src, msg) in cases {
        let mut heap = Heap::new().unwrap();
        match compile_module(&mut heap, src.as_bytes(), &CompileOptions::module("m.js")) {
            Ok(_) => panic!("{src:?} compiled"),
            Err(e) => assert!(e.message.starts_with(msg), "{src:?}: {}", e.message),
        }
    }
    // Valid forms.
    for src in [
        "import {} from 'm'; export {}",
        "import a, * as b from 'm'; export { a, b }",
        "import a, { b as c, 'd e' as f } from 'm'",
        "export { a as 'x', a as default } from 'm'; export * from 'n'",
        "export default class {}",
        "export default async function* () {}",
        "export default (function () {})",
        "export var a = 1, { b, c: [d] } = {}",
        "export async function f() { await 1 }",
        "let x; export { x as y, x as z }",
        "import.meta; () => import.meta.url",
        "label: for (;;) break label",
    ] {
        let mut heap = Heap::new().unwrap();
        if let Err(e) = compile_module(&mut heap, src.as_bytes(), &CompileOptions::module("m.js")) {
            panic!("{src:?}: {e}");
        }
    }
}
