import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Inner { Inner(uint256) }
enum Outer { Left(Inner), Right(uint256) }

impl SigString<Inner> {
  function sigStr(p: Proxy<Inner>) returns (string) { return "changed"; }
}

contract C {
  function roundtrip(value: Outer) public returns (Outer) {
    return value;
  }
}
