import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Box<a> { Box(a) }

impl SigString<Box<uint256>> {
  function sigStr(p: Proxy<Box<uint256>>) returns (string) { return "changed"; }
}

contract C {
  function inspect(value: Box<Box<Box<uint256>>>) public returns (uint256) {
    return uint256(7);
  }
}
