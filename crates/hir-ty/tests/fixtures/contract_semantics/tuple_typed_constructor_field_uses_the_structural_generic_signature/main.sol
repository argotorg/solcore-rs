pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Wrap {
    Wrap((uint256, bool))
}

contract Shapes {
    function roundtrip(value: Wrap) public returns (Wrap) {
        return value;
    }
}
