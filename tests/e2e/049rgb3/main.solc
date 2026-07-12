import std.{*};
import std.dispatch.{*};

data RGB = Red(word) | Green(word) | Blue(word);

contract RGB3 {

  function choose(c:RGB) -> word {
    let res : word;
    match c {
      | .Red(x) => assembly { res := add(x,1) }
      | .Green(x) => assembly { res := add(x,2) }
      | .Blue(x) => assembly { res := add(x,3) }
      }
      return res;
  }
  // #[() -> 44]
  public function run() -> uint256 {
    return uint256(choose(RGB.Green(42)));
  }
}
