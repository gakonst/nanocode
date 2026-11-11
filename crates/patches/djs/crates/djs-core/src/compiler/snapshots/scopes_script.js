// Script mode: top-level declarations are globals; blocks have locals.
var v = 1;
let l = 2;
const c = 3;
{
  let inner = v + l + c;
  inner = inner + 1;
  const k = inner;
  early;
  let early = k;
}
c = 4;
v = l;
this;
