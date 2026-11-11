// Literals: numbers, strings, templates, arrays, objects.
let n = [0, 1, -1, 127, 128, -129, 2147483647, 2147483648, 1.5, 0.1, -0, 0x1F, 0b101, 0o17, 1_000_000, 1e21, 1.5];
let s = ["a", 'a', "\n\t\u00e9", "😀", "", "a"];
let t = [`plain`, `x${n}y`, `${s}`, `${n}${s}`, ``, `a\u0041`];
let k = [null, true, false, undefined, this];
let holes = [, 1, , 2, ...s, 3];
let o = { a: 1, "b c": 2, 3: 3, 1.5: 4, [k]: 5, n, ...s, if: 6 };
