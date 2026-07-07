forall t. class t : Wrap {
  function unwrap(comptime x : t) -> comptime word;
}

forall t. t:Wrap => function process(z : t) -> word {
  return Wrap.unwrap(z);
}
