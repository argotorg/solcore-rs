import std.{*};
import std.dispatch.{*};
pragma no-patterson-condition ;
pragma no-coverage-condition ;
pragma no-bounded-variable-condition ;

contract Simple {
  myval : word ;

  function getVal () -> word {
    return myval ;
  }

  // #[() -> 0]
  public function run () -> uint256 {
    return uint256(getVal());
  }
}
