pragma solcore;

import * from std;
import * from std.dispatch;

// A contract whose constructor is explicitly marked `payable`.
// Deploying it with an incoming value transfer must succeed and the
// transferred value is retained by the newly created contract.
// #[scenario("PayableCtor")]
// payable constructor accepts value
// #[deploy(0x, value = 42) -> success]
// #[raw(0xb69ef8a8, label = "balance()(uint256) -- value sent at deploy was retained") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
contract PayableCtor {
    constructor() payable {}

    function balance() public returns (uint256) {
        let value;
        assembly {
            value := selfbalance()
        }
        return uint256(value);
    }
}
