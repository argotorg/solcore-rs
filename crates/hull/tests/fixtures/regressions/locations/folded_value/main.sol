pragma solcore;

import * from std;
import * from std.dispatch;

function inner() returns (word) {
    return 42;
}

function outer() returns (word) {
    return inner();
}

contract C {
    function folded(value: uint256) public returns (uint256) {
        let v: word = Typedef.rep(value);
        let k: word = outer();
        assembly {
            k := add(k, v)
        }
        return uint256(k);
    }

    function direct(value: uint256) public returns (uint256) {
        let v: word = Typedef.rep(value);
        let k: word = 7;
        assembly {
            k := add(k, v)
        }
        return uint256(k);
    }
}
