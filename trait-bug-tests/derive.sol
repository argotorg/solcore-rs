pragma solcore;

import * from std;
import * from std.Generic;

// NoEq has no Eq impl.
enum NoEq { NoEq }

// The derived impl claims `Wrap: Eq`, but its field has no Eq.
#[derive(Eq)]
enum Wrap {
    Wrap(NoEq)
}

contract K {
    function main() public returns (bool) {
        return Eq.eq(Wrap(NoEq), Wrap(NoEq));
    }
}
