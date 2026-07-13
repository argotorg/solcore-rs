import std.{*};
import std.dispatch.{*};

contract ConstructorSuffix {
  data T = A | B_A;

  function value(x:T) -> word {
    match x {
    | T.A => return 1;
    | T.B_A => return 42;
    }
  }

  // #[() -> 42]
  public function run() -> uint256 {
    return uint256(value(T.B_A));
  }
}
