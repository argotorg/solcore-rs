import * from std;
import * from std.dispatch;

export { serveTable };

enum AnswerSig { AnswerSig }

enum UnusedAbi { UnusedAbi(uint256) }

impl ABIAttribs<UnusedAbi> {
  function headSize(p: Proxy<UnusedAbi>) returns (word) { return 32; }
  function isStatic(p: Proxy<UnusedAbi>) returns (bool) { return true; }
}

impl ABIEncode<UnusedAbi> {
  function encodeInto(x: UnusedAbi, base: word, offset: word, tail: word) returns (word) {
    return tail;
  }
}

impl ABIDecode<ABIDecoder<UnusedAbi, CalldataWordReader>, UnusedAbi> {
  function decode(d: ABIDecoder<UnusedAbi, CalldataWordReader>, offset: word) returns (UnusedAbi) {
    return UnusedAbi(uint256(0));
  }
}

impl SigString<AnswerSig> {
  function sigStr(p: Proxy<AnswerSig>) returns (string) { "answer" }
}

function answer() returns (uint256) {
  return uint256(42);
}

function serveTable() {
  RunDispatch.go(Method(@AnswerSig, @NonPayable, @(), @uint256, answer));
}
