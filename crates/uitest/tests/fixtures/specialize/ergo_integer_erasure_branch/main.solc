// integer value that survives to runtime because it is chosen by a runtime
// branch: the comptime evaluator cannot fold sload, so the integer inside
// Box cannot be erased.  Judge cascade volume and span quality.
import std;

data Box = MkBox(integer);

contract IntegerEscapesBranch {
  function main() -> word {
    let v : word;
    assembly {
      v := sload(0)
    }
    let b : Box = Box.MkBox(1);
    if (v > 0) {
      b = Box.MkBox(2);
    }
    match b {
    | Box.MkBox(i) => return wordFromInteger(i);
    }
  }
}
