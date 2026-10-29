pragma solcore;

import * from std;
import * from std.dispatch;

contract A {
    function B_C(x: uint256) public returns (uint256) {
        return x;
    }
}

contract A_B {
    function C(x: uint256) public returns (uint256) {
        return x;
    }
}
