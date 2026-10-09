import * from std;
import * from std.dispatch;

impl SigString<(uint256, bool)> {
  function sigStr(p: Proxy<(uint256, bool)>) returns (string) { return "changed"; }
}

contract C {
  function inspect(value: uint256, flag: bool) public returns (uint256) {
    return value;
  }
}
