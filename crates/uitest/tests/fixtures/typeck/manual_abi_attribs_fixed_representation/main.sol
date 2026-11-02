pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Fixed {
    Fixed(bytes32)
}
enum Box<a> {
    Box(a)
}
enum Wrap<a> {
    Wrap(Box<a>)
}

// User code cannot implement std's traits for std types (SC0246). std gives
// `bytes32` its layout through a default impl, so this impl would still be
// selected and the external ABI check must find it through the derived ABI.
impl ABIAttribs<bytes32> {
    function headSize(p: Proxy<bytes32>) returns (word) {
        return 64;
    }
    function isStatic(p: Proxy<bytes32>) returns (bool) {
        return false;
    }
}

contract FixedField {
    function inspect(value: Fixed) public returns (uint256) {
        return uint256(7);
    }
}

contract NestedParameter {
    function inspect(value: Box<Wrap<bytes32>>) public returns (uint256) {
        return uint256(7);
    }
}
