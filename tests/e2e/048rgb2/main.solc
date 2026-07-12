import std.{*};
import std.dispatch.{*};

contract RGB {
  data Color = R | G | B;

  function fromEnum(c : Color) -> word {
    match c {
      | Color.R => return 4;
      | Color.G => return 2;
      | Color.B => return 42;
    }
  }

  // #[() -> 42]
  public function run() -> uint256 { return uint256(fromEnum(Color.B)); }
}
