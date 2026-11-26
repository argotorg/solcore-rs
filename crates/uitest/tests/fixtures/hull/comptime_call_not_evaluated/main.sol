pragma solcore;

// A call with a compile-time parameter must be evaluated at compile time.
// When its evaluation ends in a panic, the call is the one error.
import * from std;
import * from std.dispatch;

function atLeastTen(comptime n: integer) returns (word) {
    if (integerLt(n, 10)) {
        panic(0x01);
    }
    return wordFromInteger(n);
}

contract C {
    function accepted() public returns (word) {
        return atLeastTen(12);
    }

    function rejected() public returns (word) {
        return atLeastTen(3);
    }
}
