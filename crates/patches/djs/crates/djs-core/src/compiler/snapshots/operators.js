// Operators.
let a = 1, b = 2, c = 3;
a + b - c * a / b % c ** a ** b;
a << b >> c >>> a & b | c ^ a;
a < b; a > b; a <= b; a >= b; a == b; a != b; a === b; a !== b;
a in b; a instanceof b;
a ?? b; a || b && c; (a ?? b) || c;
a ? b : c ? a : b;
-a; +a; ~a; !a; void a; typeof a; typeof undeclared;
(a, b, c);
let o = {};
delete o.x; delete o[a]; delete 1;
a = b = c;
o.x = a; o[b] = c;
a += 1; o.x -= 2; o[a] *= 3;
a ||= b; o.x &&= c; o[b] ??= a;
++a; --o.x; ++o[b];
a++; o.x--; o[b]++;
