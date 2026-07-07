data Bool = True | False;

forall a . class a : Eq {
  function eq(x: a, y: a) -> Bool;
}

instance word : Eq {
  function eq(x: word, y: word) -> Bool {
    return Bool.True;
  }
}

function f() -> Bool {
  return Eq.eq(Bool.True, Bool.False);
}
