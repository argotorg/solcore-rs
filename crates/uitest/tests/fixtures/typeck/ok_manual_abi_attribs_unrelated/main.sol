import * from std;
import * from std.dispatch;

enum X { X(uint256) }

impl ABIAttribs<X> {
  function headSize(p: Proxy<X>) returns (word) { return 64; }
  function isStatic(p: Proxy<X>) returns (bool) { return false; }
}

contract C {
  constructor(value: uint256) {}

  function hello() public returns (uint256) { return uint256(7); }
}
