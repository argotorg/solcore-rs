pragma solcore;

import * from std;
import * from std.dispatch;

enum Tag {
    Tag(word)
}

// Only std can implement `ABIElementAttribs` (its main argument is std's
// `ABIElement`), so `Tag`'s manual element layout comes from this `ABIAttribs`
// impl through std's `ABIElementAttribs` default. The public ABI below selects
// neither manual impl.
impl ABIAttribs<Tag> {
    function headSize(p: Proxy<Tag>) returns (word) {
        return 64;
    }
    function isStatic(p: Proxy<Tag>) returns (bool) {
        return false;
    }
}
impl ABITypeString<Tag> {
    function typeStr(p: Proxy<Tag>) returns (string) {
        return "bytes32";
    }
}
function privateLayout() returns (word) {
    return ABIElementAttribs.headSize(@ABIElement<Tag>);
}
function privateSignature() returns (string) {
    return ABITypeString.typeStr(@Tag);
}
contract C {
    function take(values: calldata<array<uint256>>) public {}
}
