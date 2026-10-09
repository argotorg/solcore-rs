import * from std;
import * from std.dispatch;

export { unrelated };

enum Encoded { Encoded(uint256) }
enum Decoded { Decoded(uint256) }
enum Layout { Layout(uint256) }

impl ABIEncode<Encoded> {
  function encodeInto(value: Encoded, base: word, offset: word, tail: word) returns (word) {
    return tail;
  }
}

impl ABIDecode<ABIDecoder<Decoded, CalldataWordReader>, Decoded> {
  function decode(d: ABIDecoder<Decoded, CalldataWordReader>, offset: word) returns (Decoded) {
    return Decoded(uint256(42));
  }
}

impl ABIAttribs<Layout> {
  function headSize(p: Proxy<Layout>) returns (word) { return 64; }
  function isStatic(p: Proxy<Layout>) returns (bool) { return false; }
}

function unrelated() {}
