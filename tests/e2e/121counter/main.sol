import * from std;
import * from std.dispatch;

// test single contract field
import std;
pragma no-patterson-condition;
pragma no-coverage-condition;
pragma no-bounded-variable-condition;

contract Counter {
    counter: word;

    // #[() -> 1]
    function run() public returns (uint256) {
        counter = std.addWord(counter, 1);
        return uint256(counter);
    }
}
