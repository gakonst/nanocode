// `using` (compiler/using.rs): the Record on the stack, the region's handler and the
// disposal subroutine; `await using` awaits in a loop.
function f(a) {
  using x = a;
  for (const y of a) {
    using z = y;
    if (z) break;
  }
  return x;
}
async function g(a) {
  await using x = a;
}
