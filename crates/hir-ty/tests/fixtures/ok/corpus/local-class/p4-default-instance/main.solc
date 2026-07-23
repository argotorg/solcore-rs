enum Name { Name(word) }

trait Token<a> {
  function token(x: a) returns (word) ;
}

default impl<a> Token<a> {
  function token(x: a) returns (word) {
    return 0;
  }
}

function main() returns (word) {
  return Token.token(Name.Name(2));
}
