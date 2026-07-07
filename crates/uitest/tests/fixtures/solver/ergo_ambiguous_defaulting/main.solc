data Wrap = Wrap(word);

forall a . class a : Conv {
  function make(x: word) -> a;
  function out(y: a) -> word;
}

instance word : Conv {
  function make(x: word) -> word {
    return x;
  }
  function out(y: word) -> word {
    return y;
  }
}

instance Wrap : Conv {
  function make(x: word) -> Wrap {
    return Wrap(x);
  }
  function out(y: Wrap) -> word {
    match y {
    | Wrap(w) => return w;
    }
  }
}

function f() -> word {
  return Conv.out(Conv.make(1));
}
