pragma solcore;

import * from std;

enum meters {
    meters(word)
}

impl Eq<meters> {
    function eq(x: meters, y: meters) returns (bool) {
        match (x, y) {
            case (meters(xw), meters(yw)) {
                return eqWord(xw, yw);
            }
        }
    }
}

impl Ord<meters> {
    function gt(x: meters, y: meters) returns (bool) {
        match (x, y) {
            case (meters(xw), meters(yw)) {
                return gtWord(xw, yw);
            }
        }
    }
}

contract C {
    function main() public returns (word) {
        let a: meters = meters(1);
        let b: meters = meters(2);
        if (a < b) {
            return 42;
        } else {
            return 0;
        }
    }
}
