import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum X { X(uint256) }
enum OtherDecoder { OtherDecoder }

impl ABIDecode<OtherDecoder, X> {
  function decode(d: OtherDecoder, offset: word) returns (X) {
    return X(uint256(42));
  }
}

contract C {
  function inspect(value: X) public returns (uint256) { return uint256(7); }
}
