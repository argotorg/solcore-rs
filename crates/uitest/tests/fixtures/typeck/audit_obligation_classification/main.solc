function literal_as_callee() -> word {
  return 1();
}

function word_as_callee() -> word {
  let x: word;
  return x();
}

function from_integer_bad_arg() -> word {
  return Int.fromInteger(true);
}

forall a . function open_invokable(x: a) -> word {
  return invoke(x, ());
}
