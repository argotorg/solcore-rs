// Smuggle a runtime value into a comptime parameter through a function
// value: bind the comptime function to a local, then call the local with
// a runtime argument.  If the SAIL comptime check only looks at direct
// calls, this silently defeats the comptime contract (accept-bug).
import std;

function sloadWord() -> word {
  let v : word;
  assembly {
    v := sload(0)
  }
  return v;
}

contract CtIndirectEscape {
  function double(comptime x : word) -> comptime word {
    return x + x;
  }
  function main() -> word {
    let g = lam (y : word) { return double(y); };
    return g(sloadWord());
  }
}
