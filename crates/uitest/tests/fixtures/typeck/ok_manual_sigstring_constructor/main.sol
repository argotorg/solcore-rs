import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum InitialValue { InitialValue(uint256) }

impl SigString<InitialValue> {
  function sigStr(p: Proxy<InitialValue>) returns (string) { return "changed"; }
}

contract C {
  constructor(value: InitialValue) {}
}
