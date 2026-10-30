pragma solcore;

import * from std;
import * from std.dispatch;

// #[scenario("raw-vector", evm = "osaka")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x56377d16, label = "initialValue()") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0xfaa8aab1, label = "callerAddress()") -> success(0x0000000000000000000000001212121212121212121212121212120000000012)]
// #[raw(0x7215808b0000000000000000000000000000000000000000000000000000000000000029, label = "setStored(41)") -> success(0x)]
// #[raw(0x9e50978c, label = "valueAfterSend()") -> success(0x0000000000000000000000000000000000000000000000000000000000000029)]
contract RawVector {
    stored: uint256;

    constructor() {
        stored = uint256(7);
    }

    function initialValue() public returns (uint256) {
        return stored;
    }

    function callerAddress() public returns (address) {
        let result: word;
        assembly {
            result := caller()
        }
        return address(result);
    }

    function setStored(value: uint256) public {
        stored = value;
    }

    function valueAfterSend() public returns (uint256) {
        return stored;
    }
}
