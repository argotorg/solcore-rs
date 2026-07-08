forall t. class t : Wrap {
  function unwrap(comptime x : t) -> comptime word;
}

instance word : Wrap {
  function unwrap(comptime x : word) -> comptime word {
    return x;
  }
}

forall t. t:Wrap => function process(z : t) -> word {
  return Wrap.unwrap(z);
}

function sloadWord() -> word {
  let v : word;
  assembly {
    v := sload(0)
  }
  return v;
}

contract C {
  public function main() -> word {
    return process(sloadWord());
  }
}
