pragma solcore;

import * from std;
import * from std.dispatch;
import * from broken;

contract C {
    // #[() -> 7]
    function run() public returns (uint256) {
        return 7;
    }
}
