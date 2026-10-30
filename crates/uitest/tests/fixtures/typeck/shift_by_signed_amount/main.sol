pragma solcore;

import * from std;
import * from std.signed;

// As in Solidity, the shift amount must be unsigned: `int8` has no
// `ShiftAmount` instance.
function f(x: uint256, s: int8) returns (uint256) {
    return x << s;
}
