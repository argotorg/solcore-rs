import * from std;
import * from std.dispatch;

impl SigString<word> {
  function sigStr(p: Proxy<word>) returns (string) { return "changed"; }
}

contract C {
  function echo(value: uint256) public returns (uint256) {
    return value;
  }
}
