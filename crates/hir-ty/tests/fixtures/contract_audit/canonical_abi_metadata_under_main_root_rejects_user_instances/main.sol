pragma solcore;

import * from std;
import * from dispatch;
import * from overrides;
contract C {
    function take(values: calldata<array<calldata<array<uint256>>>>) public {}
}
