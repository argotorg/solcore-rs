import * from std;
import * from std.dispatch;

enum X { X(uint256) }

impl ABIEncode<X> {
  function encodeInto(value: X, base: word, offset: word, tail: word) returns (word) {
    return tail;
  }
}

contract C {
  constructor(value: uint256) {}

  function hello() public returns (uint256) { return uint256(7); }
}
