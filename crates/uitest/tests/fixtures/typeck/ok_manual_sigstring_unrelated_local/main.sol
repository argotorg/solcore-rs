import * from std;
import * from std.dispatch;

enum AnswerSig { AnswerSig }

impl SigString<AnswerSig> {
  function sigStr(p: Proxy<AnswerSig>) returns (string) { return "answer"; }
}

contract C {
  constructor(value: uint256) {}

  function hello() public returns (uint256) {
    return uint256(7);
  }
}
