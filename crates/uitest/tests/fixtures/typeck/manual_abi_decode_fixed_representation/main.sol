import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Fixed { Fixed(word) }
enum Box<a> { Box(a) }
enum Wrap<a> { Wrap(Box<a>) }

impl ABIDecode<ABIDecoder<word, MemoryWordReader>, word> {
  function decode(d: ABIDecoder<word, MemoryWordReader>, offset: word) returns (word) {
    return 42;
  }
}

contract FixedField {
  constructor(value: Fixed) {}
}

contract NestedParameter {
  constructor(value: Box<Wrap<word>>) {}
}
