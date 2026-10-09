import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum X { X(uint256) }

impl ABIEncode<X> {
  function encodeInto(value: X, base: word, offset: word, tail: word) returns (word) {
    return tail;
  }
}

contract C {
  constructor(value: X) {}

  function inspect(value: X) public returns (uint256) { return uint256(7); }
}
