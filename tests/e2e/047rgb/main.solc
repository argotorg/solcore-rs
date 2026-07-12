import std.{*};
import std.dispatch.{*};

contract RGB {
  data Color = R | G | B;
  // #[() -> 42]
  public function run() -> uint256 {
    match Color.B {
      | Color.R => return uint256(4);
      | Color.G => return uint256(2);
      | Color.B => return uint256(42);
    }
  }
}
