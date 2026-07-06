data Wrap = Wrap(word);

forall a . class a:Boxed {
  function unbox(x:a) -> word;
}

instance Wrap:Boxed {
  function unbox(x:Wrap) -> word {
    match x {
    | Wrap.Wrap(w) => return w;
    }
  }
}

function main() -> word {
  return Boxed.unbox(Wrap.Wrap(1));
}
