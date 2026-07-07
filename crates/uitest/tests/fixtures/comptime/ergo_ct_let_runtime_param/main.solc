// comptime let bound to a runtime function parameter: must fail comptime
// evaluation. The interesting question is span quality + cascade volume.
import std;

contract CtLetRuntimeParam {
  function scale(k : word) -> word {
    let c : comptime word = k + 1;
    return c;
  }
  function main() -> word {
    let v : word;
    assembly {
      v := sload(0)
    }
    return scale(v);
  }
}
