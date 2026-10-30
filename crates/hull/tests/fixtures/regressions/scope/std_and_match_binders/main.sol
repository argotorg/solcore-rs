pragma solcore;

import * from std;

contract C {
    x: word;

    function test(flag: bool) returns (bool) {
        return and(flag, true);
    }

    function main() public returns (bool) {
        return test(tobool(x));
    }
}
