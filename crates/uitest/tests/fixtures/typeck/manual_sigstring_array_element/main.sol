import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Elem { Elem(uint256) }

impl SigString<Elem> {
  function sigStr(p: Proxy<Elem>) returns (string) { return "changed"; }
}

contract C {
  function inspect(values: calldata<array<Elem>>) public returns (uint256) {
    return uint256(0);
  }
}
