import std.{*};
import std.dispatch.{*};

contract Option {
  data Option(a) = None | Some(a);

  function just(x : word) -> Option(word) { return Option.Some(x); }

  function maybe(n : word, o : Option(word)) -> word {
    match o {
      | Option.None => return n;
      | Option.Some(x) => return x;
    }
  }

  // #[() -> 42]
  public function run() -> uint256 {
    return uint256(maybe(0, Option.Some(42)));
  }
}
