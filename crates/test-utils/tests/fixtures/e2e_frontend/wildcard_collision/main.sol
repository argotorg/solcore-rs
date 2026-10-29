pragma solcore;

import * from std;
import * from std.dispatch;

function concatLit(x: word) returns (word) {
    return x;
}

contract C {
    // #[() -> 7]
    function run() public returns (uint256) {
        return uint256(concatLit(7));
    }
}
