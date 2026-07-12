import std.{*};
import std.dispatch.{*};

contract Triple {

  function asel(t : (word, word, word)) -> word {
    match t {
      | (a,b,c) => return c;
    }
  }

  // #[() -> 42]
  public function run() -> uint256 {
    return uint256(asel((1,21,42)));
  }
}
