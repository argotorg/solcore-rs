pragma solcore;

import * from std;
import * from std.dispatch;

contract Answer {
    function ping(x: word) public returns (word) {
        return x;
    }
}
