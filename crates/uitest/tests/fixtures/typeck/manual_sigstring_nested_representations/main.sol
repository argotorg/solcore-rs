import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum X { X(uint256) }
enum Box<a> { Box(a) }
enum Wrap<a> { Wrap(Box<a>) }

impl SigString<X> {
  function sigStr(p: Proxy<X>) returns (string) { return "changed"; }
}

contract WrapWrap {
  function inspect(value: Wrap<Wrap<X>>) public returns (uint256) {
    return uint256(7);
  }
}

contract BoxWrap {
  function inspect(value: Box<Wrap<X>>) public returns (uint256) {
    return uint256(7);
  }
}

contract BoxWrapBox {
  function inspect(value: Box<Wrap<Box<X>>>) public returns (uint256) {
    return uint256(7);
  }
}

contract BoxBox {
  function inspect(value: Box<Box<X>>) public returns (uint256) {
    return uint256(7);
  }
}
