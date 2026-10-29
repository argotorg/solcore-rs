pragma solcore;

import * from std;

contract C {
    x: word;

    function f(flag: bool) returns (word) {
        if (flag && true) {
            let x: word = 7;
        }
        return x;
    }

    function main() public returns (word) {
        return f(tobool(x));
    }
}
