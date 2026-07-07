forall a . class a : C {
  function c(x: a) -> word;
}

instance word : C {
  function c(x: word) -> word {
    return 1;
  }
}

instance word : C {
  function c(x: word) -> word {
    return 2;
  }
}

function f() -> word {
  return C.c(0);
}
