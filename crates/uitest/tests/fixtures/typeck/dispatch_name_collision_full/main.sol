pragma solcore;

import * from std;
import * from std.dispatch;

enum DispatchNameTy_C_ping {
    Collision
}

contract C {
    function ping() public returns (uint256) {
        return uint256(0);
    }
}
