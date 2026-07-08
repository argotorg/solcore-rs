forall a . class a : C {
  function f(x: a) -> word;
}

forall b . class b : D {}

instance word : C {
  forall b . b:D => function f(x: word) -> word { return x; }
}
