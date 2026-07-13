import std.{*};
import std.dispatch.{*};

data Choice = Choice(uint256);

instance Choice:Add {
  function add(l: Choice, r: Choice) -> Choice {
    return r;
  }
}

contract CompoundAssignmentClassMethod {
  constructor() {}

  // #[(3, 7) -> 7]
  // #[(11, 5) -> 5]
  public function choose_right(x: uint256, y: uint256) -> uint256 {
    let result: Choice = Choice(x);
    result += Choice(y);
    match result {
    | Choice(value) => return value;
    }
  }
}
