// test multiple contract fields
import std.{*};
import std.dispatch.{*};

contract Counter {
  counter1 : word;
  counter2 : uint256;
  counter3 : word;

  // #[() -> 3]
  public function run() -> uint256 {
    let x: word;
    x = counter1 + 1;
    counter1 = x;
    counter3 += 2;
    return uint256(counter1 + counter3);
  }
}
