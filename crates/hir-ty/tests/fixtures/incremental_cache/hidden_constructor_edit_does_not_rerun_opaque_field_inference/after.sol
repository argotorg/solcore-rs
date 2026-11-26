pragma solcore;

import { uint256, Length } from std;

export { Vec, Other };

struct Vec {
    length: uint256;
}

struct Other {
    first: word;
    second: word;
}

impl Length<Vec> {
    function length(v: Vec) returns (uint256) {
        return v.length;
    }
}
