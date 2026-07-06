data Name = Name(word);

forall a . class a:Token {
  function token(x:a) -> word;
}

forall a . default instance a:Token {
  function token(x:a) -> word {
    return 0;
  }
}

function main() -> word {
  return Token.token(Name.Name(2));
}
