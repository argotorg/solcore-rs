pragma no-coverage-condition;

forall a . class a:Parent {}
forall a b . a:Parent => class a:Child(b) {}
forall b . instance word:Child(b) {}

forall a . a:Parent => function use(x: a) -> a {
  return x;
}

forall unused . function trigger() -> word {
  return use(0);
}

function main() -> word {
  return 0;
}
