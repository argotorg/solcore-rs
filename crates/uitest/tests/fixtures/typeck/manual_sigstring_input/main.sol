import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Input { Input(uint256) }

impl SigString<Input> {
  function sigStr(p: Proxy<Input>) returns (string) { return "changed"; }
}

contract C {
  function echo(value: Input) public returns (Input) {
    return value;
  }
}
