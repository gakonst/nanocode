// goal: body
// for-of and for-in (docs/compiler.md 3.8, compiler/for_in_of.rs).
// A declaration head: the store sits next to the body.
for (const x of xs) console.log(x);
// var and expression heads.
for (var v of xs) ;
for (o.p of xs) ;
for (o[k] of xs) ;
// break closes the iterator inline; continue does not.
for (const x of xs) { if (x) break; else continue; }
// Leaving through labels and return closes through the subroutine.
outer: for (const x of xs) { for (const y of ys) { break outer; } }
function f() { for (const x of xs) { return x; } }
// Per-iteration bindings captured by closures.
for (let x of xs) fns.push(() => x);
// for-in.
for (const k in o) console.log(k);
for (o.k in o) { if (k) break; }
