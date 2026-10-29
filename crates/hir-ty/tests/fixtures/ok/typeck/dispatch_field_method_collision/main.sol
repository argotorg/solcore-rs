pragma solcore;

import * from std;
import * from std.dispatch;

contract C {
    enum C {
        Foo
    }

    allowance: uint256;

    function allowance() public returns (uint256) {
        return allowance;
    }

    function Foo() public returns (uint256) {
        return 1;
    }
}
