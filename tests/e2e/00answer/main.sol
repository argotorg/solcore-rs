pragma solcore;

import * from std;
import * from std.dispatch;

contract Answer {
    // #[() -> 42]
    function run() public returns (uint256) {
        return uint256(42);
    }
}
