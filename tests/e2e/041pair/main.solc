import std.{*};
import std.dispatch.{*};

contract Pair {

  function fst(p : (word, word)) -> word {
    match p {
      | (a,b) => return a;
    }
  }

  // #[() -> 1]
  public function run() -> uint256 {
    return uint256(fst((1,0)));
  }
}
