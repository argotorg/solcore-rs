import std.{*};
import std.dispatch.{*};

contract RevertExpectation {
  // #[(7) -> revert(0xdeadbeef)]
  public function fail(x: uint256) -> uint256 {
    assembly {
      mstore(0, 0xdeadbeef)
      revert(28, 4)
    }
    return x;
  }
}
