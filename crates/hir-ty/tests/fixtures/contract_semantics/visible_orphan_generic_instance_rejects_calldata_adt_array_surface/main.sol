pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;
import * from model;

impl Generic<Payload, word> {}

contract C {
    function inspect(values: calldata<array<Payload>>) public returns (uint256) {
        return uint256(0);
    }
}
