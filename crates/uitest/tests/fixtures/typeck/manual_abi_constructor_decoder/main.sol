import * from std;
import * from std.dispatch;

impl ABIDecode<ABIDecoder<word, MemoryWordReader>, word> {
  function decode(d: ABIDecoder<word, MemoryWordReader>, offset: word) returns (word) {
    return 42;
  }
}

contract C {
  constructor(value: word) {}
}
