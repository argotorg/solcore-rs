data Bool = True | False;

forall a . class a : Same {
  function same(x: a, y: a) -> Bool;
}

forall a . function f(x: a) -> Bool {
  return Same.same(x, x);
}
