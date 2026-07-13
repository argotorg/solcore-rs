// Non-terminating comptime recursion: inline-depth exhaustion must stop the
// recursive evaluator before the larger total-work fuel budget is consumed.
import std;

function spin(comptime n : integer) -> comptime integer {
  return spin(integerAdd(n, 1));
}

contract CtFuelInfinite {
  function main() -> word {
    return wordFromInteger(spin(0));
  }
}
