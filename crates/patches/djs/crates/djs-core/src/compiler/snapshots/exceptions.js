// goal: body
// try/catch/finally: handler tables, gosub/ret, return/break/continue through finally.
function f() {
  try { return g(); } finally { h(); }
}
for (let i = 0; i < 3; i++) {
  try {
    if (i) continue;
    break;
  } catch (e) {
    throw e;
  } finally {
    log(i);
  }
}
outer: while (true) {
  try { try { break outer; } finally { a(); } } finally { b(); }
}
try { x(); } catch { y(); }
try { x(); } catch (err) { let inner = err; } finally { z(); }
