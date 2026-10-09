import * from std;
import * from std.dispatch;

impl SigString<()> {
  function sigStr(p: Proxy<()>) returns (string) { return "changed"; }
}

contract C {
  function hello() public returns (uint256) {
    return uint256(7);
  }
}
