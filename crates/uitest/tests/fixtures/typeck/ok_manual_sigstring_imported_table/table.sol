import * from std;
import * from std.dispatch;

export { serveTable };

enum AnswerSig { AnswerSig }

impl SigString<AnswerSig> {
  function sigStr(p: Proxy<AnswerSig>) returns (string) { "answer" }
}

function answer() returns (uint256) {
  return uint256(42);
}

function serveTable() {
  RunDispatch.go(Method(@AnswerSig, @NonPayable, @(), @uint256, answer));
}
