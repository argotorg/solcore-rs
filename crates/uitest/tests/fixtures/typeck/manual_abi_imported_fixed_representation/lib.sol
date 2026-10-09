import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

export { Fixed, Decoded };

enum Fixed { Fixed(bool) }
enum Decoded { Decoded(word) }

impl ABIAttribs<bool> {
  function headSize(p: Proxy<bool>) returns (word) { return 64; }
  function isStatic(p: Proxy<bool>) returns (bool) { return false; }
}

impl ABIDecode<ABIDecoder<word, MemoryWordReader>, word> {
  function decode(d: ABIDecoder<word, MemoryWordReader>, offset: word) returns (word) {
    return 42;
  }
}
