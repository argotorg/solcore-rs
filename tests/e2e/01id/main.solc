import std.{*};
import std.dispatch.{*};

contract Id1 {

  data Bool = False | True;

  function id(x : word) -> word {
    return x ;
  }

  function const(x : word, y : Bool) -> word { return x; }

  // #[() -> 42]
  public function run() -> uint256 {
    return uint256(const(id(42), Bool.False));
  }
}
