pragma solcore;

import * from std;

contract DirectWriter {
    x: word;

    function setv(v: word) returns (word) {
        x = v;
        return v;
    }

    function main() public returns (word) {
        let a: word = setv(9);
        return x;
    }
}
