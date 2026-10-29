pragma solcore;

import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

export { Booking(*), seatsOf, costOf };

// An unset mapping entry is Empty, the first variant.
enum Booking {
    Empty,
    Active(uint256, uint256)
}

function seatsOf(booking: Booking) returns (uint256) {
    // TODO 2a: match Empty or Active and return the seat count.
    return uint256(0);
}

function costOf(booking: Booking) returns (uint256) {
    // TODO 2b: match Empty or Active and return the quoted cost.
    return uint256(0);
}
