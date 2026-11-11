// Classes (0040): definition, fields and the initializer function, private names,
// a derived constructor with super(), super properties.
class A extends Object {
  x = 1;
  #p = 2;
  static s = 3;
  constructor(v) {
    super(v);
  }
  #m() {
    return this.#p;
  }
  get g() {
    return super.toString() + this.#m();
  }
  static {
    this.t = 4;
  }
  static has(o) {
    return #p in o;
  }
}
