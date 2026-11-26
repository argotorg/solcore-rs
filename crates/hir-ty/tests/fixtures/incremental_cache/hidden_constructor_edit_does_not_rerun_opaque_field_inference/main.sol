pragma solcore;

import { Vec, Other } from util;
import { uint256, Length } from std;

function size(v: Vec) returns (uint256) {
    return v.length;
}
