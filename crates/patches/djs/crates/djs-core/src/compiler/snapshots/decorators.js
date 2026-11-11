// Decorators and auto-accessors (0065): the decorator arrays in hidden bindings, the
// decoration after the body, the class decorators before the inner binding.
@cls
class C {
  @dec m() {}
  @o.dec static [k]() {}
  @dec #p() {}
  @dec f = 1;
  @dec accessor a;
  accessor #b = 2;
}
