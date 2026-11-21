pragma solcore;
import * from std;
import * from std.dispatch;

contract C {
    function twice(x: uint256) public returns (uint256) {
        return x + x;
    }
}
