import std.{*};
import std.dispatch.{*};

contract Id1 {
  function id(x : word) -> word {
    return x ;
  }

  function nid(x : word) -> word {
    return id(x);
  }

  function const(x : word, y : word) -> word { return x; }

  // #[() -> 42]
  public function run() -> uint256 {
    return uint256(const(nid(42), id(1)));
  }
}
