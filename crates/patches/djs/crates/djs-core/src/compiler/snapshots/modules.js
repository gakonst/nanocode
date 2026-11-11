// goal: module
import d, { a, b as c, "s t" as e } from "./dep.js";
import * as ns from "./dep.js";
import "./side.js";
export { x as "y z", f };
export * from "./star.js";
export * as all from "./all.js";
export { q as r, default as dd } from "./dep.js";
export { a as reA, ns as reNs };
var x = 1;
let l = a + c + e + d;
export const k = () => l + x + ns.z + import.meta.url;
function f() { return l; }
export default function () { return f(); }
export class C {}
this;
