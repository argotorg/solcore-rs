pragma no-patterson-condition ;

data Box(a) = MkBox(a);

forall a . class a : C {
  function c(x: a) -> word;
}

forall a . Box(a) : C => instance a : C {
  function c(x: a) -> word {
    return 1;
  }
}

function f() -> word {
  return C.c(0);
}
