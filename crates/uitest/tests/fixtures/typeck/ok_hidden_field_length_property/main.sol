pragma solcore;

import * from std;
import { Vec, mk } from lib;

function size() returns (uint256) {
    let v: Vec = mk();
    return v.length;
}
