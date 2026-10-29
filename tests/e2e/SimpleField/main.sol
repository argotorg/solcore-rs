pragma solcore;

import * from std;
import * from std.dispatch;
pragma no-patterson-condition;
pragma no-coverage-condition;
pragma no-bounded-variable-condition;

contract Simple {
    myval: word;

    function getVal() returns (word) {
        return myval;
    }

    // #[() -> 0]
    function run() public returns (uint256) {
        return uint256(getVal());
    }
}
