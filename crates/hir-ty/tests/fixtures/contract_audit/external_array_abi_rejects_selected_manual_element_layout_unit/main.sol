pragma solcore;

import * from std;
import * from std.dispatch;
impl ABIElementAttribs<ABIElement<()>> {
    function headSize(p: Proxy<ABIElement<()>>) returns (word) {
        return 64;
    }
}
contract C {
    function take(values: calldata<array<()>>) public returns (uint256) {
        return uint256(0);
    }
}
