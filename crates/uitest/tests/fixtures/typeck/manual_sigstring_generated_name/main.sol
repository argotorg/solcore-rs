import * from std;
import * from std.dispatch;

impl SigString<DispatchNameTy_C_hello> {
  function sigStr(p: Proxy<DispatchNameTy_C_hello>) returns (string) { return "changed"; }
}

contract C {
  function hello() public returns (uint256) {
    return uint256(7);
  }
}
