// Object literals: methods, accessors, computed names, __proto__, spread; calls with spread.
const k = "dyn";
const o = {
  a: 1,
  m(x) { return this.a + x; },
  get g() { return 1; },
  set s(v) {},
  [k]: () => 2,
  [k + "m"]() {},
  get [k]() { return 3; },
  __proto__: null,
  __proto__x: 1,
  ...k,
};
f(...o, 1, ...o);
o.m(...[1]);
new F(...o);
delete o?.a;
delete o?.b.c;
delete o?.[k];
