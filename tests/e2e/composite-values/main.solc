import std.{*};
import std.dispatch.{*};

contract CompositeValues {
  constructor() {}

  // #[((7, 1), 9) -> (7, 1, 9)]
  public function pack(point: (uint256, uint256), tag: uint256) -> ((uint256, uint256), uint256) {
    return (point, tag);
  }

  // #[((7, 1, 9)) -> (7, 1, 9)]
  public function unpack(tagged: ((uint256, uint256), uint256)) -> ((uint256, uint256), uint256) {
    return tagged;
  }

  // #[((0, 0, 0)) -> (0, 0, 0)]
  // #[((42, 1, 99)) -> (42, 1, 99)]
  public function echo(tagged: ((uint256, uint256), uint256)) -> ((uint256, uint256), uint256) {
    return tagged;
  }
}
