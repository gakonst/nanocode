#!/usr/bin/env python3
"""Rewrites the INTRINSICS table in crates/djs-core/src/bytecode/intrinsics.rs from
builtins/ (0079, D1791).

The same rules as that file's `from_sources` test (which stays the check): the
`declare function`s of builtins/intrinsics.d.ts (@leaf or @reentrant), then the
functions tagged @intrinsic in builtins/*.ts, files in name order. It cannot be the test
itself: when the table is out of date, djs-prelude's build script (a dev-dependency of
djs-core's tests) fails before the test runs.
"""
import os
import re
import sys

root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
builtins = os.path.join(root, "builtins")
table = os.path.join(root, "crates/djs-core/src/bytecode/intrinsics.rs")


def doc_before(src, at):
    before = src[:at].rstrip()
    if not before.endswith("*/"):
        return ""
    s = before.rfind("/**")
    return before[s:] if s >= 0 else ""


def arity(params):
    n, variadic = 0, False
    for p in (p.strip() for p in params.split(",")):
        if not p:
            continue
        if p.startswith("..."):
            variadic = True
        elif not (p.startswith("this:") or p.startswith("this :")):
            n += 1
    return n, variadic


def entries():
    out = []
    dts = open(os.path.join(builtins, "intrinsics.d.ts"), encoding="utf-8").read()
    for m in re.finditer("declare function ", dts):
        rest = dts[m.end():]
        name = rest[: rest.index("(")]
        params = rest[len(name) + 1 : rest.index(")")]
        doc = doc_before(dts, m.start())
        if "@reentrant" in doc:
            kind = "R"
        elif "@leaf" in doc:
            kind = "L"
        else:
            sys.exit(f"gen-intrinsics: {name}: no @leaf/@reentrant tag")
        out.append((name, kind) + arity(params))
    files = sorted(f for f in os.listdir(builtins) if f.endswith(".ts") and not f.endswith(".d.ts"))
    for f in files:
        src = open(os.path.join(builtins, f), encoding="utf-8").read()
        for m in re.finditer("\nfunction ", src):
            at = m.start() + 1
            if "@intrinsic" not in doc_before(src, at):
                continue
            rest = src[at + len("function ") :]
            name = rest[: rest.index("(")]
            params = rest[len(name) + 1 : rest.index(")")]
            out.append((name, "B") + arity(params))
    return out


def main():
    lines = open(table, encoding="utf-8").read().split("\n")
    start = next(i for i, l in enumerate(lines) if l.startswith("pub const INTRINSICS:"))
    end = next(i for i in range(start, len(lines)) if lines[i] == "];")
    new = [
        f'    i("{n}", {k}, {p}, {"true" if v else "false"}),' for n, k, p, v in entries()
    ]
    if lines[start + 1 : end] != new:
        lines[start + 1 : end] = new
        open(table, "w", encoding="utf-8").write("\n".join(lines))
        print("gen-intrinsics: rewrote the INTRINSICS table")


if __name__ == "__main__":
    main()
