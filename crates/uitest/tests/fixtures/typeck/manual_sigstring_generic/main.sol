import * from std;
import * from std.dispatch;

impl<a> SigString<a> {
  function sigStr(p: Proxy<a>) returns (string) { return "changed"; }
}

contract C {
  function echo(value: uint256) public returns (uint256) {
    return value;
  }
}
