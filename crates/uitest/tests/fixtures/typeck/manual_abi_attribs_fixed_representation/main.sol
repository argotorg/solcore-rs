import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Fixed { Fixed(bool) }
enum Box<a> { Box(a) }
enum Wrap<a> { Wrap(Box<a>) }

impl ABIAttribs<bool> {
  function headSize(p: Proxy<bool>) returns (word) { return 64; }
  function isStatic(p: Proxy<bool>) returns (bool) { return false; }
}

contract FixedField {
  function inspect(value: Fixed) public returns (uint256) {
    return uint256(7);
  }
}

contract NestedParameter {
  function inspect(value: Box<Wrap<bool>>) public returns (uint256) {
    return uint256(7);
  }
}
