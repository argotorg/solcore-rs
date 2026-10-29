pragma solcore;

import * from std;
import * from std.dispatch;
impl ABITypeString<uint256> {
    function typeStr(p: Proxy<uint256>) returns (string) {
        return "bytes32";
    }
}
contract C {
    function take(values: calldata<array<calldata<array<uint256>>>>) public {}
}
