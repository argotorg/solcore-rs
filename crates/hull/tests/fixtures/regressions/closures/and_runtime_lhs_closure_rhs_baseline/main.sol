pragma solcore;

import * from std;
import * from std.dispatch;

function twice(f: function (uint256) returns (uint256), x: uint256) returns (uint256) {
    return f(f(x));
}

contract C {
    function run(x: uint256) public returns (uint256) {
        return twice(lam (v: uint256) -> uint256 {
                return v;
            }, x);
    }
}
