pragma solcore;

import * from std;
import * from std.dispatch;

contract WithFallback {
    function answer() public returns (uint256) {
        return uint256(42);
    }

    fallback() {
        revertLit("fallback-was-called");
    }
}
