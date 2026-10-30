pragma solcore;

import * from std;
import * from dispatch;
export {};
impl ABIElementAttribs<ABIElement<uint256>> {
    function headSize(p: Proxy<ABIElement<uint256>>) returns (word) {
        return 64;
    }
}
impl ABITypeString<uint256> {
    function typeStr(p: Proxy<uint256>) returns (string) {
        return "bytes32";
    }
}
