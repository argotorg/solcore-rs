import std.{*};
import std.dispatch.{*};
contract Counter {
  counter : word;

  // #[() -> 42]
  public function run() -> uint256 {
    counter = Num.add(counter, 42);
    return uint256(counter);
  }
}
