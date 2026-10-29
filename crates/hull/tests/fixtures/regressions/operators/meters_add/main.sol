pragma solcore;

import * from std;

enum meters {
    meters(word)
}

impl Add<meters> {
    function add(x: meters, y: meters) returns (meters) {
        match (x, y) {
            case (meters(xw), meters(yw)) {
                return meters(addWord(xw, yw));
            }
        }
    }
}

function unwrap(x: meters) returns (word) {
    match (x) {
        case meters(w) {
            return w;
        }
    }
}

contract C {
    function main() public returns (word) {
        let a: meters = meters(1);
        let b: meters = meters(2);
        let c: meters = a + b;
        return unwrap(c);
    }
}
