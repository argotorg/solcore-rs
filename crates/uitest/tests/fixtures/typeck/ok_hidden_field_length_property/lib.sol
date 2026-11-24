pragma solcore;

import * from std;

export { Vec, mk };

struct Vec {
    length: uint256;
}

function mk() returns (Vec) {
    return Vec(uint256(3));
}

impl Length<Vec> {
    function length(v: Vec) returns (uint256) {
        return v.length;
    }
}
