pragma solcore;

// The returns of one function hold different function values, so which one
// the caller receives is decided at run time.
import * from std;
import * from std.dispatch;

function inc(v: uint256) returns (uint256) {
    return v + 1;
}

function dbl(v: uint256) returns (uint256) {
    return v * 2;
}

function choose(c: bool) returns (function (uint256) returns (uint256)) {
    if (c) {
        return inc;
    }
    return dbl;
}

contract C {
    function run(c: bool, x: uint256) public returns (uint256) {
        let f: function (uint256) returns (uint256) = choose(c);
        return f(x);
    }
}
