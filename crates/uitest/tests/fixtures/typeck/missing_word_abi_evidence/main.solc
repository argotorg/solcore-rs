import std.{*};
import std.dispatch.{*};

// `word` has ABI metadata (`uint256`) but the pinned shared std does not yet
// provide its selector/decode/encode evidence. The frontend must terminate
// with a bounded solver diagnostic while that evidence is missing.
contract WordAbiProbe {
  public function echo(value: word) -> word {
    return value;
  }
}
