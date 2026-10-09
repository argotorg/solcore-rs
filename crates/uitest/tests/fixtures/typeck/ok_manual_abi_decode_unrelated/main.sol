import * from std;
import * from std.dispatch;

enum X { X(uint256) }

impl ABIDecode<ABIDecoder<X, CalldataWordReader>, X> {
  function decode(d: ABIDecoder<X, CalldataWordReader>, offset: word) returns (X) {
    return X(uint256(42));
  }
}

contract C {
  constructor(value: uint256) {}

  function hello() public returns (uint256) { return uint256(7); }
}
