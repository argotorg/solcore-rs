pragma solcore;

import * from std;
import * from std.dispatch;
impl ABIElementAttribs<ABIElement<address>> {
    function headSize(p: Proxy<ABIElement<address>>) returns (word) {
        return 64;
    }
}
impl ABITypeString<address> {
    function typeStr(p: Proxy<address>) returns (string) {
        return "bytes32";
    }
}
function privateLayout() returns (word) {
    return ABIElementAttribs.headSize(@ABIElement<address>);
}
function privateSignature() returns (string) {
    return ABITypeString.typeStr(@address);
}
contract C {
    function take(values: calldata<array<uint256>>) public {}
}
