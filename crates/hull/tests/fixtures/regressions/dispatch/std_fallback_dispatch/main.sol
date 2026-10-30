pragma solcore;

import * from std;
import * from std.dispatch;

contract C {
    function answer() public returns (uint256) {
        return uint256(42);
    }
    fallback() {}
}
