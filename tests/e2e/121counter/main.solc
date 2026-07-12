import std.{*};
import std.dispatch.{*};

// test single contract field
import std;
pragma no-patterson-condition ;
pragma no-coverage-condition ;
pragma no-bounded-variable-condition ;

contract Counter {
  counter : word;

  // #[() -> 1]
  public function run() -> uint256 {
    counter = std.addWord(counter, 1);
    return uint256(counter);
  }
}
