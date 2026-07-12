import std.{*};
import std.dispatch.{*};

contract Option {
  data Option(a) = None | Some(a);

  function maybe(n : word, o : Option(word)) -> word {
    match o {
      | Option.Some(x) => return x;
      | _ => return n;
    }
  }

  // #[() -> 7]
  public function run() -> uint256 {
    return uint256(maybe(7, Option.None));
  }
}
