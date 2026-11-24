pragma solcore;

import * from std;
import * from std.dispatch;
import { Box } from lib;

contract C {
    box: Box;

    function read() public returns (uint256) {
        return box.value;
    }

    function write() public {
        box.value = 1;
    }
}
