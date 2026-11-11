//! Suspension maps (compiler/smap.rs, D2563): the temporaries per construct.

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

use super::{CompileOptions, suspension_maps};
use crate::bytecode::template::disasm_template;
use crate::heap::Heap;

/// The temporaries of each site by construct, as one line per
/// site (`-` for none, `!construct` for an unsupported site).
fn constructs(src: &str) -> Vec<String> {
    let mut heap = Heap::new().unwrap();
    let opts = CompileOptions::async_body("t.js", &[]);
    let maps = suspension_maps(&mut heap, src.as_bytes(), &opts).unwrap();
    let mut out = Vec::new();
    for m in &maps {
        for s in &m.sites {
            out.push(match &s.unsupported {
                Some(u) => alloc::format!("{} !{u}", s.name),
                None => {
                    // Temporaries in locals, then on the stack, by construct.
                    let mut r: Vec<String> = s
                        .locals
                        .iter()
                        .filter_map(|x| match x {
                            super::Slot::Temp(n) => n.split('@').next().map(String::from),
                            _ => None,
                        })
                        .collect();
                    r.extend(
                        s.stack
                            .iter()
                            .filter_map(|n| n.split('@').next())
                            .map(String::from),
                    );
                    if r.is_empty() {
                        alloc::format!("{} -", s.name)
                    } else {
                        alloc::format!("{} {}", s.name, r.join(" "))
                    }
                }
            });
        }
    }
    out
}

/// `(source, the temporaries of each site)` for each construct that keeps values.
const CONSTRUCTS: &[(&str, &[&str])] = &[
    // Calls: callee and `this`, then the arguments so far.
    (
        "f(await a, await b);",
        &[
            "await#1 call.callee call.this",
            "await#2 call.callee call.this call.arg",
        ],
    ),
    (
        "o.m(a, await b).n(await c);",
        &[
            "await#1 call.callee call.this call.arg",
            "await#2 call.callee call.this",
        ],
    ),
    (
        "f(...xs, await y);",
        &["await#1 call.callee call.this call.args"],
    ),
    ("a?.b(await c);", &["await#1 call.callee call.this"]),
    ("f?.(await x);", &["await#1 call.callee call.this"]),
    ("(a?.b)(await x);", &["await#1 call.callee call.this"]),
    (
        "tag`a${x}b${await c}`;",
        &["await#1 call.callee call.this call.arg call.arg"],
    ),
    (
        "await f(await g(await h()));",
        &[
            "await#1 -",
            "await#2 call.callee call.this",
            "await#3 call.callee call.this call.callee call.this",
        ],
    ),
    ("new X(a, await b);", &["await#1 new.callee new.arg"]),
    ("new X(...(await xs));", &["await#1 new.callee new.args"]),
    ("new (await X)();", &["await#1 -"]),
    ("import(a, await b);", &["await#1 import.specifier"]),
    // Members and assignments.
    ("o[await k];", &["await#1 member.object"]),
    ("o?.[await k];", &["await#1 member.object"]),
    ("delete o[await k];", &["await#1 member.object"]),
    ("o.p = await v;", &["await#1 assign.object"]),
    ("o[k] = await v;", &["await#1 assign.object assign.key"]),
    (
        "x.y = z.w = await q;",
        &["await#1 assign.object assign.object"],
    ),
    ("x += await v;", &["await#1 assign.old"]),
    ("o.p += await v;", &["await#1 assign.object assign.old"]),
    ("o[k] ??= await v;", &["await#1 assign.object assign.key"]),
    ("x = y = await z;", &["await#1 -"]),
    // Operators: only the ones that keep their left operand.
    ("a + await b;", &["await#1 binary.left"]),
    (
        "a = b + (c * await d);",
        &["await#1 binary.left binary.left"],
    ),
    ("a in await b;", &["await#1 binary.left"]),
    ("a && await b;", &["await#1 -"]),
    ("c ? await a : b;", &["await#1 -"]),
    ("(await a, await b);", &["await#1 -", "await#2 -"]),
    // Literals.
    ("[a, await b];", &["await#1 array.element"]),
    ("[, await b];", &["await#1 array.literal"]),
    ("[...xs, await y];", &["await#1 array.literal"]),
    (
        "({ a: 1, [k]: await v });",
        &["await#1 object.literal object.key"],
    ),
    ("({ [await k]: 1 });", &["await#1 object.literal"]),
    ("({ a, ...(await o) });", &["await#1 object.literal"]),
    (
        "x = { y: [1, await z] };",
        &["await#1 object.literal array.element"],
    ),
    (
        "`${a}: ${await b}`;",
        &["await#1 template.part template.part"],
    ),
    ("`${await a}`;", &["await#1 -"]),
    // Generators.
    (
        "function* h() { const v = yield 1; yield [v, yield 2]; }",
        &["yield#1 -", "yield#2 -", "yield#3 array.element"],
    ),
    (
        "async function* g() { f(yield x); }",
        &[
            "yield.await#1 call.callee call.this",
            "yield#1 call.callee call.this",
            "yield.return#1 call.callee call.this",
        ],
    ),
    (
        "async function* g() { return await x; }",
        &["return.await#1 -", "await#1 -"],
    ),
];

/// The loops' iterator records.
const LOOPS: &[(&str, &[&str])] = &[
    (
        "for (const x of xs) await x;",
        &["await#1 for-of.iterator for-of.next"],
    ),
    ("for (const k in o) await k;", &["await#1 for-in.iterator"]),
    (
        "for await (const x of xs) await x;",
        &[
            "for-await.close#1 for-await.exception",
            "for-await.next#1 for-await.iterator for-await.next",
            "await#1 for-await.iterator for-await.next",
        ],
    ),
    (
        "for (const x of xs) { for (const k in x) { f(await k); } }",
        &["await#1 for-of.iterator for-of.next for-in.iterator call.callee call.this"],
    ),
    // Loops whose code is rotated: the sites follow their instructions.
    (
        "for (let i = 0; i < await n; i++) { await i; }",
        &["await#1 binary.left", "await#2 -"],
    ),
    ("while (await a) { await b; }", &["await#1 -", "await#2 -"]),
    // Delegation: the inner iterator and its `next` below every suspension.
    (
        "function* h() { f(yield* xs); }",
        &["yield*#1 yield*.iterator yield*.next call.callee call.this"],
    ),
    (
        "async function* h() { yield* xs; }",
        &[
            "yield*#1 yield*.iterator yield*.next",
            "yield*.await#1 yield*.iterator yield*.next",
            "yield*.await#2 -",
            "yield*.await#3 -",
            "yield*.await#4 -",
            "yield*.await#5 yield*.iterator yield*.next",
            "yield*.await#6 yield*.iterator yield*.next",
            "yield*.await#7 yield*.iterator yield*.next",
        ],
    ),
    (
        "do { await b; } while (await a);",
        &["await#1 -", "await#2 -"],
    ),
];

/// Case tests, finally blocks and a for-await's close on a jump out (their completions
/// in temporaries, D2733).
const STATEMENTS: &[(&str, &[&str])] = &[
    (
        "switch (a) { case await b: break; }",
        &["await#1 binary.left"],
    ),
    ("switch (a) { case 1: await b; }", &["await#1 -"]),
    (
        "try { a } finally { await b }",
        &["await#1 completion.kind completion.value"],
    ),
    (
        "try { await a } catch (e) { await e }",
        &["await#1 -", "await#2 -"],
    ),
    (
        "for (const x of xs) { try { await x } finally { f(await y) } }",
        &[
            "await#1 for-of.iterator for-of.next",
            "await#2 for-of.iterator for-of.next completion.kind completion.value call.callee call.this",
        ],
    ),
    (
        "try { try {} finally { await a } } finally { await b }",
        &[
            "await#1 completion.kind completion.value",
            "await#2 completion.kind completion.value",
        ],
    ),
    (
        "for await (const x of xs) { if (x) return 1; }",
        &[
            "for-await.close#1 for-await.exception",
            "for-await.next#1 for-await.iterator for-await.next",
            "for-await.close#2 completion.value",
        ],
    ),
];

/// `using` keeps its resource Record in a temporary; the disposal is a finally block.
const USING: &[(&str, &[&str])] = &[
    (
        "{ await using r = open(); await r.x; }",
        &[
            "await#1 using.record",
            "await-using#1 using.record completion.kind completion.value",
        ],
    ),
    (
        "for (await using r of rs) { f(await r); }",
        &[
            "await#1 for-of.iterator for-of.next using.record call.callee call.this",
            "await-using#1 for-of.iterator for-of.next using.record completion.kind completion.value",
        ],
    ),
];

/// Destructuring: the value or iterator under way, keys, assignment targets.
const PATTERNS: &[(&str, &[&str])] = &[
    // A pattern's state is listed while a later step reads it.
    ("const { a = await b } = o;", &["await#1 -"]),
    ("const { a = await b, c } = o;", &["await#1 pattern.source"]),
    ("const [x] = await y;", &["await#1 -"]),
    (
        "const { a, [await k]: { b = await c }, ...r } = o;",
        &[
            "await#1 pattern.source pattern.excluded",
            "await#2 pattern.source pattern.excluded pattern.key",
        ],
    ),
    (
        "const [x = await a, [y = await b] = []] = xs;",
        &[
            "await#1 pattern.iterator pattern.next",
            "await#2 pattern.iterator pattern.iterator",
        ],
    ),
    (
        "[o[await k], p.q = await d] = xs;",
        &[
            "await#1 pattern.iterator pattern.next assign.value member.object",
            "await#2 pattern.iterator assign.value assign.object",
        ],
    ),
    (
        "({ x: o[k] = await d } = v);",
        &["await#1 assign.value assign.object assign.key"],
    ),
    (
        "for (const { a = await d } of xs) f(a);",
        &["await#1 for-of.iterator for-of.next"],
    ),
    ("try {} catch ({ e = await d }) {}", &["await#1 -"]),
];

/// Constructs whose operands are not described yet: never mislabeled.
const UNSUPPORTED: &[(&str, &[&str])] = &[("class C { [await k]() {} }", &["await#1 !class"])];

fn check(table: &[(&str, &[&str])]) {
    let mut bad = Vec::new();
    for (src, want) in table {
        let got = constructs(src);
        if got.iter().map(String::as_str).collect::<Vec<_>>() != *want {
            bad.push(alloc::format!("{src}\n  want {want:?}\n  got  {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn temporaries_per_construct() {
    check(CONSTRUCTS);
}

#[test]
fn loop_records() {
    check(LOOPS);
}

#[test]
fn statements() {
    check(STATEMENTS);
}

#[test]
fn using_records() {
    check(USING);
}

#[test]
fn patterns() {
    check(PATTERNS);
}

#[test]
fn unsupported_constructs() {
    check(UNSUPPORTED);
}

/// The slot names of each site (D2721), as one line per site.
fn names(src: &str) -> Vec<String> {
    let mut heap = Heap::new().unwrap();
    let opts = CompileOptions::async_body("t.js", &[]);
    let maps = suspension_maps(&mut heap, src.as_bytes(), &opts).unwrap();
    let mut out = Vec::new();
    for m in &maps {
        for s in &m.sites {
            out.push(alloc::format!("{} {}", s.name, s.temporaries().join(" ")));
        }
    }
    out
}

/// Slots are named by construct and the offset of what keeps the value, from the
/// function's start (the async body's wrapper adds none: its text starts at the
/// source's first byte, here `0`).
#[test]
fn slot_names_per_construct() {
    let table: &[(&str, &[&str])] = &[
        // callee and `this` by the call's start, arguments by their own.
        (
            "o.m(a, await b);",
            &["await#1 call.callee@0 call.this@0 call.arg@4"],
        ),
        (
            "f(...xs, await y);",
            &["await#1 call.callee@0 call.this@0 call.args@1"],
        ),
        (
            "x = [1, f(await y)];",
            &["await#1 array.element@5 call.callee@8 call.this@8"],
        ),
        ("a + await b;", &["await#1 binary.left@0"]),
        (
            "o[k] += await v;",
            &["await#1 assign.object@0 assign.key@0 assign.old@0"],
        ),
        (
            "x = `a${b}${await c}`;",
            &["await#1 template.part@4 template.part@8"],
        ),
        (
            "for (const x of xs) await x;",
            &["await#1 for-of.iterator@0 for-of.next@0"],
        ),
        // The discriminant's temporary is read again only by a later test; the test
        // under way keeps its copy (the left side of its `===`).
        (
            "switch (d) { case 1: break; case await t: }",
            &["await#1 binary.left@28"],
        ),
        (
            "switch (d) { case await t: break; case 2: }",
            &["await#1 switch.discriminant@0 binary.left@13"],
        ),
    ];
    let mut bad = Vec::new();
    for (src, want) in table {
        let got = names(src);
        if got.iter().map(String::as_str).collect::<Vec<_>>() != *want {
            bad.push(alloc::format!("{src}\n  want {want:?}\n  got  {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// No two operands of a site share a name, in every construct the tables cover.
#[test]
fn slot_names_are_unique_per_site() {
    for table in [CONSTRUCTS, LOOPS, STATEMENTS, USING, PATTERNS] {
        for (src, _) in table {
            let mut heap = Heap::new().unwrap();
            let opts = CompileOptions::async_body("t.js", &[]);
            for m in suspension_maps(&mut heap, src.as_bytes(), &opts).unwrap() {
                for s in &m.sites {
                    for (i, n) in s.stack.iter().enumerate() {
                        assert!(!s.stack[..i].contains(n), "{src}: {} {n} twice", s.name);
                    }
                }
            }
        }
    }
}

/// The stack's names come from where the compiler emits the construct: a lowering that
/// pushed `this` under the callee gives another map, which the pin test would show.
#[test]
fn a_callee_this_swap_changes_the_map() {
    let src = "f(await x);";
    assert_eq!(constructs(src), ["await#1 call.callee call.this"]);
    let mut heap = Heap::new().unwrap();
    let opts = CompileOptions::async_body("t.js", &[]);
    let (_, maps) = super::suspension_maps_with(&mut heap, src.as_bytes(), &opts, |p| {
        p.test_swap_call = true;
    })
    .unwrap();
    assert_eq!(maps[0].sites[0].stack, ["call.this@0", "call.callee@0"]);
}

/// Every slot: bindings by name and declaring scope (nothing outside it), hidden
/// locals by name, arguments.
#[test]
fn slots_name_bindings_and_hidden_locals() {
    let src = "async function f(a, b) {\n  let x = 1;\n  { let y = 2; await a; }\n  await b;\n}";
    let mut heap = Heap::new().unwrap();
    let maps = suspension_maps(&mut heap, src.as_bytes(), &CompileOptions::script("t.js")).unwrap();
    assert_eq!(
        maps[0].text(src.as_bytes()),
        "function f @0..76\n  arg0 a@0, arg1 b@0\n  loc0 <state>, loc1 x@0, loc2 y@42\n  \
         node 0 Body @0..76: Params 16..22 Stmt 27..37 Stmt 40..63 Stmt 66..74 | \
         Param a@0#0 Param b@0#0 Let x@0#1\n  \
         node 1 Block @40..63: Stmt 42..52 Stmt 53..61 | Let y@42#0\n  \
         node 2 Expr @53..61: Operand 53..60\n  node 3 Await @53..60: Operand 59..60\n  \
         node 4 Expr @66..74: Operand 66..73\n  node 5 Await @66..73: Operand 72..73\n  \
         await#1 @53 (3:16) pc 11: -\n    path 0.2 1.1 2.0 3\n  \
         await#2 @66 (4:3) pc 16: - (out: loc2)\n    path 0.3 4.0 5\n"
    );
    let y = super::Slot::Binding {
        name: String::from("y"),
        scope: 42,
    };
    assert_eq!(maps[0].sites[0].locals[2], y);
    assert_eq!(maps[0].sites[1].locals[2], super::Slot::Nothing);
    assert_eq!(
        maps[0].sites[0].locals[0],
        super::Slot::Hidden(String::from("<state>"))
    );
}

/// A site's resume pc is right after the suspending instruction of the code a normal
/// compile produces.
#[test]
fn resume_pcs_follow_the_suspending_instruction() {
    use crate::bytecode::Op;
    use crate::bytecode::decode::insns;
    let src = "const r = [];\nfor (const x of xs) { r.push(await f(x)); }\nif (await g()) { r.push(1); }\n";
    let mut heap = Heap::new().unwrap();
    let opts = CompileOptions::async_body("t.js", &[]);
    let t = super::compile(&mut heap, src.as_bytes(), &opts).unwrap();
    let code = crate::bytecode::template::code(&heap, t.0).to_vec();
    let ends: Vec<(u32, Op)> = insns(&code)
        .map(|i| i.unwrap())
        .map(|i| (i.next_pc(), i.op))
        .collect();
    let maps = suspension_maps(&mut heap, src.as_bytes(), &opts).unwrap();
    assert_eq!(maps[0].sites.len(), 2);
    for s in &maps[0].sites {
        assert!(
            ends.contains(&(s.resume_pc, Op::Await)),
            "{} at {}",
            s.name,
            s.resume_pc
        );
    }
}

/// The pinned maps (D2563): every await and yield of the codemode corpora and the e2e
/// tests, as text next to the cases' names in tests/smap/. A change is a diff to accept
/// deliberately (`DJS_UPDATE_SMAPS=1`): dumps stopped at a changed site by an older
/// build would be refused. tests/smap/summary.txt counts the sites by role sequence: it is
/// derived from the pins, so `DJS_UPDATE_SMAP_SUMMARY=1` rewrites only it and still checks
/// the pins (scripts/regen.sh after a merge, which keeps one side of it: 0214).
#[test]
fn pinned_maps() {
    use std::path::{Path, PathBuf};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let update = std::env::var_os("DJS_UPDATE_SMAPS").is_some();
    let update_summary = update || std::env::var_os("DJS_UPDATE_SMAP_SUMMARY").is_some();
    // (directory, compiled as the difftest's codemode arrow?)
    let areas = [
        ("tests/corpus/codemode", "codemode", true),
        ("tests/corpus/codemode-harvest", "codemode-harvest", true),
        ("tests/data/e2e", "e2e", false),
    ];
    let mut failed = Vec::new();
    let mut seen = Vec::new();
    let mut sites = 0u32;
    let mut unsupported: std::collections::BTreeMap<String, u32> = Default::default();
    let mut sequences: std::collections::BTreeMap<String, u32> = Default::default();
    let mut kinds: std::collections::BTreeMap<String, u32> = Default::default();
    // Paths (D2850): complete ones, and gaps by the step whose part holds them.
    let mut complete = 0u32;
    let mut gaps: std::collections::BTreeMap<String, u32> = Default::default();
    for (dir, area, corpus) in areas {
        let mut files: Vec<PathBuf> = std::fs::read_dir(root.join(dir))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "js" || e == "ts"))
            .collect();
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let ts = path.extension().is_some_and(|e| e == "ts");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let (src, opts_body) = if corpus {
                // As the difftest runs a case (crates/djs-difftest/src/djs.rs).
                (
                    alloc::format!("(async (tools, console) => {{{text}\n}})"),
                    false,
                )
            } else {
                (text.clone(), text.starts_with("// mode: body"))
            };
            let legacy = text.contains("// difftest: experimental-decorators");
            let metadata = text.contains("// difftest: emit-decorator-metadata");
            let params = ["tools", "console"];
            let mut opts = if opts_body {
                CompileOptions::async_body(&name, &params)
            } else {
                CompileOptions::script(&name)
            };
            opts.typescript = ts;
            opts.experimental_decorators = legacy || metadata;
            opts.emit_decorator_metadata = metadata;
            let mut heap = Heap::new().unwrap();
            let Ok((t, maps)) =
                super::suspension_maps_with(&mut heap, src.as_bytes(), &opts, |_| {})
            else {
                continue;
            };
            // Recording changes nothing in the code.
            let mut plain = Heap::new().unwrap();
            let t2 = super::compile(&mut plain, src.as_bytes(), &opts).unwrap();
            if disasm_template(&heap, t) != disasm_template(&plain, t2) {
                failed.push(alloc::format!("{name}: recording changed the code"));
            }
            let maps: Vec<_> = maps.into_iter().filter(|m| !m.sites.is_empty()).collect();
            if maps.is_empty() {
                continue;
            }
            let mut out = String::new();
            for m in &maps {
                out.push_str(&m.text(src.as_bytes()));
                for s in &m.sites {
                    sites += 1;
                    for (i, n) in s.stack.iter().enumerate() {
                        if s.stack[..i].contains(n) {
                            failed.push(alloc::format!("{name}: {} names {n} twice", s.name));
                        }
                    }
                    *kinds.entry(String::from(s.kind.name())).or_default() += 1;
                    if let Some(e) = path_error(m, s) {
                        failed.push(alloc::format!("{name}: {} {e}", s.name));
                    }
                    match s.gap {
                        None => complete += 1,
                        Some(g) => {
                            let at = s
                                .path
                                .iter()
                                .rev()
                                .find_map(|st| {
                                    let n = &m.syntax.nodes[st.node as usize];
                                    let p = n.parts.get(st.part? as usize)?;
                                    (p.start <= g && g < p.end)
                                        .then(|| alloc::format!("{}.{:?}", n.kind.name(), p.role))
                                })
                                .unwrap_or_else(|| String::from("?"));
                            *gaps.entry(at).or_default() += 1;
                        }
                    }
                    match &s.unsupported {
                        Some(u) => *unsupported.entry(u.clone()).or_default() += 1,
                        None => {
                            // Temporaries in locals, then on the stack, by construct.
                            let mut seq: Vec<String> = s
                                .locals
                                .iter()
                                .filter_map(|x| match x {
                                    super::Slot::Temp(n) => n.split('@').next().map(String::from),
                                    _ => None,
                                })
                                .collect();
                            seq.extend(
                                s.stack
                                    .iter()
                                    .filter_map(|n| n.split('@').next())
                                    .map(String::from),
                            );
                            let seq = if seq.is_empty() {
                                String::from("-")
                            } else {
                                seq.join(" ")
                            };
                            *sequences.entry(seq).or_default() += 1;
                        }
                    }
                }
            }
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            let pin = root
                .join("tests/smap")
                .join(area)
                .join(alloc::format!("{stem}.smap"));
            seen.push(pin.clone());
            if update {
                std::fs::create_dir_all(pin.parent().unwrap()).unwrap();
                std::fs::write(&pin, &out).unwrap();
            } else if std::fs::read_to_string(&pin).unwrap_or_default() != out {
                failed.push(alloc::format!("{}", pin.display()));
            }
        }
    }
    let mut summary =
        alloc::format!("# Suspension map coverage (compiler/smap_tests.rs)\nsites {sites}\n");
    summary.push_str("\n## by kind\n");
    for (k, n) in &kinds {
        summary.push_str(&alloc::format!("{n:5} {k}\n"));
    }
    summary.push_str("\n## described, by temporaries (in locals, then on the stack)\n");
    let mut seqs: Vec<_> = sequences.iter().collect();
    seqs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (s, n) in seqs {
        summary.push_str(&alloc::format!("{n:5} {s}\n"));
    }
    summary.push_str(&alloc::format!(
        "\n## paths (D2850)\n{complete:5} complete\n\n## path gaps, by the part they are in\n"
    ));
    for (g, n) in &gaps {
        summary.push_str(&alloc::format!("{n:5} {g}\n"));
    }
    summary.push_str("\n## unsupported\n");
    for (u, n) in &unsupported {
        summary.push_str(&alloc::format!("{n:5} {u}\n"));
    }
    let summary_path = root.join("tests/smap/summary.txt");
    if update {
        std::fs::write(&summary_path, &summary).unwrap();
        // Pins of cases that no longer have sites (or are gone).
        for area in ["codemode", "codemode-harvest", "e2e"] {
            for e in std::fs::read_dir(root.join("tests/smap").join(area)).unwrap() {
                let p = e.unwrap().path();
                if !seen.contains(&p) {
                    std::fs::remove_file(p).unwrap();
                }
            }
        }
        return;
    }
    if update_summary {
        std::fs::write(&summary_path, &summary).unwrap();
    } else if std::fs::read_to_string(&summary_path).unwrap_or_default() != summary {
        failed.push(alloc::format!("{}", summary_path.display()));
    }
    for area in ["codemode", "codemode-harvest", "e2e"] {
        if let Ok(dir) = std::fs::read_dir(root.join("tests/smap").join(area)) {
            for e in dir {
                let p = e.unwrap().path();
                if !seen.contains(&p) {
                    failed.push(alloc::format!("{} (stale)", p.display()));
                }
            }
        }
    }
    assert!(
        failed.is_empty(),
        "suspension maps changed (DJS_UPDATE_SMAPS=1 to accept; dumps stopped at a \
         changed site by an older build will be refused):\n{}",
        failed.join("\n")
    );
}

/// Each site's path (D2850) as `Kind.Role` steps, the last as its kind, with ` gap`
/// when the path misses a construct.
fn paths(src: &str) -> Vec<String> {
    paths_as(src, false)
}

fn paths_as(src: &str, ts: bool) -> Vec<String> {
    let mut heap = Heap::new().unwrap();
    let mut opts = CompileOptions::async_body("t.js", &[]);
    opts.typescript = ts;
    let maps = suspension_maps(&mut heap, src.as_bytes(), &opts).unwrap();
    let mut out = Vec::new();
    for m in &maps {
        for s in &m.sites {
            let mut steps = Vec::new();
            for st in &s.path {
                let n = &m.syntax.nodes[st.node as usize];
                steps.push(match st.part {
                    Some(p) => alloc::format!("{}.{:?}", n.kind.name(), n.parts[p as usize].role),
                    None => n.kind.name(),
                });
            }
            let mut line = alloc::format!("{} {}", s.name, steps.join(" "));
            if s.gap.is_some() {
                line.push_str(" gap");
            }
            out.push(line);
        }
    }
    out
}

/// `(source, each site's path)`: the statements and expressions around sites.
const PATHS: &[(&str, &[&str])] = &[
    (
        "f(a, await b);",
        &["await#1 Body.Stmt Expr.Operand Call.Arg Await"],
    ),
    (
        "o?.m?.(await x)[await y];",
        &[
            "await#1 Body.Stmt Expr.Operand Member[].Object Call?.Arg Await",
            "await#2 Body.Stmt Expr.Operand Member[].Property Await",
        ],
    ),
    (
        "new C(...a, await b); tag`x${await c}`; import(await d);",
        &[
            "await#1 Body.Stmt Expr.Operand New.Arg Await",
            "await#2 Body.Stmt Expr.Operand TaggedTemplate.Subst Await",
            "await#3 Body.Stmt Expr.Operand ImportCall.Arg Await",
        ],
    ),
    (
        "const x = 1, y = await f(x); var z = await g; let w;",
        &[
            "await#1 Body.Stmt Decl(Const).Init Await",
            "await#2 Body.Stmt Decl(Var).Init Await",
        ],
    ),
    (
        "for (const a of xs) { if (a) { o.m(await g(a)); } else h(); }",
        &["await#1 Body.Stmt ForOf.Body Block.Stmt If.Then Block.Stmt Expr.Operand Call.Arg Await"],
    ),
    (
        "if (a) b; else if (await c) d; else await e;",
        &[
            "await#1 Body.Stmt If.Else If.Test Await",
            "await#2 Body.Stmt If.Else If.Else Expr.Operand Await",
        ],
    ),
    (
        "x.y = await z; ++o[await k]; (await p).q++; v += await w;",
        &[
            "await#1 Body.Stmt Expr.Operand Assign.Right Await",
            "await#2 Body.Stmt Expr.Operand Update.Property Await",
            "await#3 Body.Stmt Expr.Operand Update.Object Await",
            "await#4 Body.Stmt Expr.Operand Assign.Right Await",
        ],
    ),
    (
        "await a + b * await c; d ?? await e; f ? await g : h; (i, await j); !await k;",
        &[
            "await#1 Body.Stmt Expr.Operand Binary.Left Await",
            "await#2 Body.Stmt Expr.Operand Binary.Right Binary.Right Await",
            "await#3 Body.Stmt Expr.Operand Logical.Right Await",
            "await#4 Body.Stmt Expr.Operand Conditional.Then Await",
            "await#5 Body.Stmt Expr.Operand Sequence.Expr Await",
            "await#6 Body.Stmt Expr.Operand Unary.Operand Await",
        ],
    ),
    (
        "try { await a } catch (e) { let q = await e; } finally { await f }",
        &[
            "await#1 Body.Stmt Try.Block Block.Stmt Expr.Operand Await",
            "await#2 Body.Stmt Try.CatchBody Block.Stmt Decl(Let).Init Await",
            "await#3 Body.Stmt Try.Finally Block.Stmt Expr.Operand Await",
        ],
    ),
    (
        "switch (await x) { case await y: let z = 2; await z; default: }",
        &[
            "await#1 Body.Stmt Switch.Discriminant Await",
            "await#2 Body.Stmt Switch.Case Await",
            "await#3 Body.Stmt Switch.Stmt Expr.Operand Await",
        ],
    ),
    (
        "a: b: while (true) { do { `a${await b}c`; } while (await d) }",
        &[
            "await#1 Body.Stmt Label.Body Label.Body While.Body Block.Stmt DoWhile.Body \
             Block.Stmt Expr.Operand Template.Subst Await",
            "await#2 Body.Stmt Label.Body Label.Body While.Body Block.Stmt DoWhile.Test Await",
        ],
    ),
    (
        "for (let i = await n; i < await m; i += await k) [await x, , ...y];",
        &[
            "await#1 Body.Stmt For.Init Decl(Let).Init Await",
            "await#2 Body.Stmt For.Test Binary.Right Await",
            "await#3 Body.Stmt For.Update Assign.Right Await",
            "await#4 Body.Stmt For.Body Expr.Operand Array.Element Await",
        ],
    ),
    (
        "({a: await b, [await k]: 1, ...await s});",
        &[
            "await#1 Body.Stmt Expr.Operand Object.Value Await",
            "await#2 Body.Stmt Expr.Operand Object.ComputedKey Await",
            "await#3 Body.Stmt Expr.Operand Object.Spread Await",
        ],
    ),
    (
        "for await (const v of await g()) { (async () => await 1)(); }",
        &[
            "for-await.close#1 Body.Stmt ForAwait",
            "for-await.next#1 Body.Stmt ForAwait",
            "await#1 Body.Stmt ForAwait.Iterable Await",
            "await#1 Body.Operand Await",
        ],
    ),
    (
        "for (x in await o) await x; return await r;",
        &[
            "await#1 Body.Stmt ForIn.Iterable Await",
            "await#2 Body.Stmt ForIn.Body Expr.Operand Await",
            "await#3 Body.Stmt Return.Operand Await",
        ],
    ),
    // Patterns (D2852).
    (
        "const {a = await b} = c;",
        &["await#1 Body.Stmt Decl(Const).Target ObjectPattern.Shorthand Default.Init Await"],
    ),
    (
        "const [x, , [y = await z] = await w, ...r] = v;",
        &[
            "await#1 Body.Stmt Decl(Const).Target ArrayPattern.Element Default.Target \
             ArrayPattern.Element Default.Init Await",
            "await#2 Body.Stmt Decl(Const).Target ArrayPattern.Element Default.Init Await",
        ],
    ),
    (
        "({[await k]: a, b: o[await i] = 1, ...r} = v);",
        &[
            "await#1 Body.Stmt Expr.Operand Assign.Left ObjectPattern.ComputedKey Await",
            "await#2 Body.Stmt Expr.Operand Assign.Left ObjectPattern.Value Default.Target \
             Member[].Property Await",
        ],
    ),
    (
        "for (const [a = await d] of xs); try {} catch ({e = await f}) {}",
        &[
            "await#1 Body.Stmt ForOf.Head ArrayPattern.Element Default.Init Await",
            "await#2 Body.Stmt Try.CatchParam ObjectPattern.Shorthand Default.Init Await",
        ],
    ),
    (
        "async function* gen() { yield await 1; throw await 2; }",
        &[
            "yield.await#1 Body.Stmt Expr.Operand Yield",
            "yield#1 Body.Stmt Expr.Operand Yield",
            "yield.return#1 Body.Stmt Expr.Operand Yield",
            "await#1 Body.Stmt Expr.Operand Yield.Operand Await",
            "await#2 Body.Stmt Throw.Operand Await",
        ],
    ),
];

#[test]
fn site_paths() {
    for (src, want) in PATHS {
        assert_eq!(paths(src), *want, "{src}");
    }
}

/// TypeScript expressions are transparent (D2853): no node, no gap.
#[test]
fn site_paths_through_typescript() {
    assert_eq!(
        paths_as(
            "const x: number = (await y) as T; f((await z)!, <U>await w satisfies V);",
            true
        ),
        [
            "await#1 Body.Stmt Decl(Const).Init Await",
            "await#2 Body.Stmt Expr.Operand Call.Arg Await",
            "await#3 Body.Stmt Expr.Operand Call.Arg Await",
        ]
    );
}

/// Declarations land on the node of their scope with the part that declares them;
/// special references and spans are relative to the function.
#[test]
fn site_syntax_text() {
    let src = "async function f(a, {b}) { var v; let w = 1; for (const [k, x] of a) \
               { o.p = `${k}${await x}`; } try { } catch (e) { const q = 1; await q; } \
               function g() {} return arguments.length + new.target; }";
    let mut heap = Heap::new().unwrap();
    let maps = suspension_maps(&mut heap, src.as_bytes(), &CompileOptions::script("t.js"));
    let m = &maps.unwrap()[0];
    let text = m.syntax.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "  node 0 Body @0..196: Params 16..24 Stmt 27..33 Stmt 34..44 Stmt 45..96 \
             Stmt 97..140 FuncDecl 141..156 Stmt 157..194 | Param a@0#0 Param b@0#0 \
             Var v@27#1 Let w@27#2 Function g@27#5",
            "  node 1 ForOf @45..96: Head 50..62 Iterable 66..67 Body 69..96 | \
             Const k@56#0 Const x@56#0",
            "  node 2 Block @69..96: Stmt 71..94",
            "  node 3 Expr @71..94: Operand 71..93",
            "  node 4 Assign @71..93: Object 71..72 Property 73..74 Op 75..76 Right 77..93",
            "  node 5 Template @77..93: Quasi 77..80 Subst 80..81 Quasi 81..84 Subst 84..91 \
             Quasi 91..93",
            "  node 6 Await @84..91: Operand 90..91",
            "  node 7 Try @97..140: Block 101..104 CatchParam 112..113 CatchBody 115..140 | \
             CatchParam e@111#1",
            "  node 8 Block @115..140: Stmt 117..129 Stmt 130..138 | Const q@111#0",
            "  node 9 Expr @130..138: Operand 130..137",
            "  node 10 Await @130..137: Operand 136..137",
            "  special Arguments@164 NewTarget@183",
        ]
    );
}

/// What is wrong with a site's path (D2850), if anything: it must start at the body,
/// name the part that holds each next step, and end at the site's own construct.
fn path_error(m: &super::FunctionMap, s: &super::Site) -> Option<String> {
    use super::{NodeKind, SiteKind};
    let node = |i: u32| &m.syntax.nodes[i as usize];
    let first = s.path.first()?;
    if node(first.node).kind != NodeKind::Body {
        return Some(String::from("path does not start at the body"));
    }
    let (last, inner) = s.path.split_last()?;
    if last.part.is_some() || inner.iter().any(|st| st.part.is_none()) && s.gap.is_none() {
        return Some(String::from("a step without its part"));
    }
    let n = node(last.node);
    let want: &[NodeKind] = match s.kind {
        SiteKind::Await => &[NodeKind::Await],
        SiteKind::Yield
        | SiteKind::AsyncYield
        | SiteKind::AsyncYieldAwait
        | SiteKind::AsyncYieldReturn => &[NodeKind::Yield],
        SiteKind::YieldStar | SiteKind::YieldStarAwait => &[NodeKind::YieldStar],
        SiteKind::ForAwaitNext | SiteKind::ForAwaitClose => &[NodeKind::ForAwait],
        SiteKind::AsyncReturn => &[NodeKind::Return],
        SiteKind::AwaitUsing => &[
            NodeKind::Block,
            NodeKind::Body,
            NodeKind::Switch,
            NodeKind::For,
            NodeKind::ForOf,
            NodeKind::ForAwait,
        ],
    };
    if !want.contains(&n.kind) {
        return Some(alloc::format!("path ends at {}", n.kind.name()));
    }
    let at = match s.kind {
        SiteKind::AwaitUsing => true,
        // (At the end of the `yield*` expression.)
        SiteKind::YieldStarAwait => n.start <= s.pos && s.pos <= n.end,
        _ => n.start == s.pos,
    };
    if !at {
        return Some(alloc::format!("path ends at {}, not at the site", n.start));
    }
    None
}
