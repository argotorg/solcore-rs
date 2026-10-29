pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

// A missing signature dictionary must never fall back to structural 0/1 tags.
enum Choice {
    First(uint256),
    Second(uint256)
}

function main() returns (word) {
    return ABIEncode.encodeInto(Choice.First(uint256(7)), 128, 0, 192);
}
