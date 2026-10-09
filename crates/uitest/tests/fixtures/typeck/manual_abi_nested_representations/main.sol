import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum X { X(uint256) }
enum Box<a> { Box(a) }
enum Wrap<a> { Wrap(Box<a>) }

impl ABIEncode<X> {
  function encodeInto(value: X, base: word, offset: word, tail: word) returns (word) {
    return tail;
  }
}

contract C {
  function inspect(value: Box<Wrap<X>>) public returns (Box<Wrap<X>>) {
    return value;
  }
}
