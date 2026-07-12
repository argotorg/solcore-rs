// test multiple contract fields
import std.{*};
import std.dispatch.{*};
// import StorageLib;


contract Counter {
  counter1 : word;
  counter2 : uint256;
  counter3 : word;
  // #[() -> 3]
  public function run() -> uint256 {
    counter1 += 1;
    counter3 += 2;
    return uint256(counter1 + counter3);
  }
}
