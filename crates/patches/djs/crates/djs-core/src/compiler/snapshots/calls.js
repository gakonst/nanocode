// Member access, calls, new, optional chains.
let o = { a: { b: 1 } };
o.a.b; o["a"]["b"]; o[1]; o["0"];
f(); f(1, 2, 3); o.m(); o["m"](1); o[0](); (o.m)(); (0, o.m)();
new X; new X(1, 2); new o.C(); new new X()();
o?.a; o?.[1]; o?.m(); o.m?.(); f?.(); o?.a.b.c(1);
console.log("hello", o.a?.b ?? 2);
