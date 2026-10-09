import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Answer { Answer(uint256) }

impl SigString<Answer> {
  function sigStr(p: Proxy<Answer>) returns (string) { return "changed"; }
}

contract C {
  function hello() public returns (Answer) {
    return Answer(uint256(7));
  }
}
