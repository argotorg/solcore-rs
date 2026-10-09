import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Unrelated { Unrelated(uint256) }
enum Phantom<a> { Phantom(uint256) }

impl SigString<Unrelated> {
  function sigStr(p: Proxy<Unrelated>) returns (string) { return "changed"; }
}

contract C {
  function inspect(value: Phantom<Unrelated>) public returns (uint256) {
    return uint256(0);
  }
}
