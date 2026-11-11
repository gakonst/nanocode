// Control flow.
let x = 0;
if (x) x = 1;
if (x) { x = 2 } else if (x > 1) { x = 3 } else if (x > 2) x = 4; else x = 5;
while (x < 10) x++;
do { x-- } while (x > 0)
for (let i = 0; i < 3; i++) { if (i == 1) continue; if (i == 2) break; x += i }
for (;;) { break }
for (var j = 0; j < 2;) j++;
outer: for (let i = 0; i < 2; i++) { inner: while (true) { if (i) continue outer; break inner } }
block: { if (x) break block; x = 7 }
switch (x) {
  case 1: x = 10;
  case 2: x = 20; break;
  default: x = 30;
  case 3: { x = 40 }
}
switch (x) {}
throw x;
debugger;
