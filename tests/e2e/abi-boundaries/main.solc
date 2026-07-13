import std.{*};
import std.dispatch.{*};

contract AbiBoundaries {
  // #[(0) -> 0]
  // #[(0x8000000000000000000000000000000000000000000000000000000000000000) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  public function echoUint(value: uint256) -> uint256 {
    return value;
  }

  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 1) -> 0]
  public function wrappingAdd(lhs: uint256, rhs: uint256) -> uint256 {
    return lhs + rhs;
  }

  // #[(0xffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffff]
  public function echoAddress(value: address) -> address {
    return value;
  }

  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  public function echoBytes32(value: bytes32) -> bytes32 {
    return value;
  }
}
