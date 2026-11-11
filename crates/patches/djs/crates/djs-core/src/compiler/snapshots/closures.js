// goal: body
// Closures: a counter, captured loop variables, three levels, hoisting.
function counter() {
  let n = 0;
  return () => ++n;
}
const fns = [];
for (let i = 0; i < 3; i++) fns[i] = () => i;
function outer(a) {
  let b = 1;
  return function middle() {
    return (c) => a + b + c + middle.length;
  };
}
isEven(4);
function isEven(n) { return n == 0 ? true : isOdd(n - 1); }
function isOdd(n) { return n == 0 ? false : isEven(n - 1); }
