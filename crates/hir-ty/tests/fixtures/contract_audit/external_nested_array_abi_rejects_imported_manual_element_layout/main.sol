pragma solcore;

import * from std;
import * from std.dispatch;
import * from layout;
contract C {
    function take(values: calldata<array<calldata<array<uint256>>>>) public {}
}
