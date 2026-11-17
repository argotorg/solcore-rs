pragma solcore;

import * from std;

enum NoEq { NoEq }

// std.Generic is not imported.
#[derive(Eq)]
enum Wrap {
    Wrap(NoEq)
}

contract K {
    function main() public returns (bool) {
        return Eq.eq(Wrap(NoEq), Wrap(NoEq));
    }
}
