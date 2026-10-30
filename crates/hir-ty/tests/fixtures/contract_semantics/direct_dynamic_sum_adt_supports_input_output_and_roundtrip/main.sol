pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum D2 {
    L(uint256),
    R(memory<bytes>)
}

contract SumRoundtrip {
    function rtD2(x: D2) public returns (D2) {
        return x;
    }
}
