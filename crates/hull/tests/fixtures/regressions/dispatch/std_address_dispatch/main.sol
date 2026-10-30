pragma solcore;

import * from std;
import * from std.dispatch;

contract C {
    function id_address(a: address) public returns (address) {
        return a;
    }
}
