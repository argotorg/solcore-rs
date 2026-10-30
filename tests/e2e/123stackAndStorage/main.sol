pragma solcore;

// test multiple contract fields
import * from std;
import * from std.dispatch;

contract Counter {
    counter1: word;
    counter2: uint256;
    counter3: word;

    // #[() -> 3]
    function run() public returns (uint256) {
        let x: word;
        x = counter1 + 1;
        counter1 = x;
        counter3 += 2;
        return uint256(counter1 + counter3);
    }
}
