pragma solcore;

import * from std;

enum uint {
    u(word)
}

impl Add<uint> {
    function add(x: uint, y: uint) returns (uint) {
        return uint.u(42);
    }
}

function unwrap(x: uint) returns (word) {
    match (x) {
        case uint.u(w) {
            return w;
        }
    }
}

contract C {
    function main() public returns (word) {
        let a: uint = uint.u(1);
        let b: uint = uint.u(2);
        let c: uint = a + b;
        return unwrap(c);
    }
}
