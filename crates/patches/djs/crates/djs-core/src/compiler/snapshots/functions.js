// Functions: params, defaults, rest, this, arguments, new.target, names.
function f(a, b = a + 1, ...rest) { return a + b + rest.length; }
const g = function () { return this; };
const h = (x) => { return arguments; };
function k() { const arrow = () => [this, arguments, new.target]; return arrow(); }
function late(a = b, b) {}
let named = function self(n) { return n ? self(n - 1) : 0; };
var obj = { m: function () { return this.v; }, a: () => 1 };
new f(1);
