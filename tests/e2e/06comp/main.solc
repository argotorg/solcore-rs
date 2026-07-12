import std.{*};
import std.dispatch.{*};

contract Compose {
  function id(x : word) -> word { return x; }

  function idid(x : word) -> word { return id(id(x)); }

  // #[() -> 42]
  public function run() -> uint256 {
    return uint256(idid(42));
  }
}
