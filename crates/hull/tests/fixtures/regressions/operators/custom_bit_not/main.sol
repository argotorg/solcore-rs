pragma solcore;

import * from std;

enum mask {
    mask(word)
}

impl BitNot<mask> {
    function bnot(x: mask) returns (mask) {
        return mask(42);
    }
}

function unwrap(x: mask) returns (word) {
    match (x) {
        case mask(w) {
            return w;
        }
    }
}

contract C {
    function main() public returns (word) {
        return unwrap(~mask(0));
    }
}
