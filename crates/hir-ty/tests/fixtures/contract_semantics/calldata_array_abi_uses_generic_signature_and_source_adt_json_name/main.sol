pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Operation {
    Approve(uint256),
    Reject(uint256)
}

contract Batch {
    function count(ops: calldata<array<Operation>>) public returns (uint256) {
        return uint256(0);
    }
}
