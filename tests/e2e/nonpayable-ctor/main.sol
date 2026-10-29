pragma solcore;

import * from std;
import * from std.dispatch;

// A contract whose constructor is NOT marked `payable`. Deploying it with an
// incoming value transfer must revert with the NonPayableReceivedValue error
// (selector 0xb5988ea3), exactly like calling a non-payable method with value.
// #[scenario("NonPayableCtor")]
// non-payable constructor rejects incoming value
// #[deploy(0x, value = 42) -> revert(0xb5988ea3)]
contract NonPayableCtor {
    constructor() {}

    function balance() public returns (uint256) {
        let value;
        assembly {
            value := selfbalance()
        }
        return uint256(value);
    }
}
