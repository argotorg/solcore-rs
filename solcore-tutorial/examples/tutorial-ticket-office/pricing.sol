pragma solcore;

import * from std;

export { Standard(*), Premium(*), Priced, quote };

enum Standard {
    Standard
}

enum Premium {
    Premium
}

trait Priced<a> {
    function unitPrice(ticket: a) returns (uint256);
}

impl Priced<Standard> {
    function unitPrice(ticket: Standard) returns (uint256) {
        return uint256(10);
    }
}

impl Priced<Premium> {
    function unitPrice(ticket: Premium) returns (uint256) {
        return uint256(15);
    }
}

// The caller bounds quantity before computing the total.
function quote<a>(ticket: a, quantity: uint256) returns (uint256) where a: Priced {
    return Priced.unitPrice(ticket) * quantity;
}
