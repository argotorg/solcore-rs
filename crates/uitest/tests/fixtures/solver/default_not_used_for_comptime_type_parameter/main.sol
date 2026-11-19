pragma solcore;

import { Fee, charge } from fees;

enum B {
    B
}

impl Fee<B> {
    function fee(x: B) returns (word) {
        return 1;
    }
}

function viaGeneric() returns (word) {
    let x: comptime<B> = B.B;
    return charge(x);
}
