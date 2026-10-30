pragma solcore;

import * from std;
import * from std.dispatch;
impl ABIElementAttribs<ABIElement<calldata<array<uint256>>>> {
    function headSize(p: Proxy<ABIElement<calldata<array<uint256>>>>) returns (word) {
        return 64;
    }
}
contract C {
    function take(values: calldata<array<calldata<array<uint256>>>>) public returns (uint256) {
        return uint256(0);
    }
}
