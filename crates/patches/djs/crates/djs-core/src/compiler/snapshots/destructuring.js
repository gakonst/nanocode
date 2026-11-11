// goal: body
// Destructuring (compiler/pattern.rs): the value runs before the pattern; object
// patterns pick the value and read fields; array patterns keep `next it` and step with
// pick/poke; the close handler covers everything but the steps.
let {a, b: [c = 1, ...d], ...rest} = x;
[a, o.p] = [o.p, a];
function f({k}, [m] = []) { return k + m; }
for (const [key, value] of entries) use(key, value);
tag`one${a}two`;
