// Async functions, generators and for await (docs/compiler.md 7.7, compiler/generator.rs).
// An async function: async_start, the body in a catch-all region, async_return.
async function f(a = 1) {
  try {
    return await g(a);
  } finally {
    log();
  }
}
// An async arrow with an expression body.
const h = async (x) => x + (await x);
// A generator: generator_start after the parameters; a yield inside an expression
// returns through the one-instruction throw region on return(x).
function* gen(n = 0) {
  const r = 1 + (yield n);
  return r;
}
// yield*: the delegation loop with yield_star.
function* deleg(it) {
  return yield* it;
}
// An async generator: yield awaits its operand and a return(x) resumption.
async function* agen(xs) {
  for await (const x of xs) yield x;
}
