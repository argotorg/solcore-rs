import std.{*};
import std.dispatch.{*};

function add(x : word, y : word) -> word {
  let res: word;
  assembly {
     res := add(x, y)
  }
  return res;
}

contract Add1 {
  // #[() -> 42]
  public function run() -> uint256 {
    return uint256(add(40, 2));
  }
}
