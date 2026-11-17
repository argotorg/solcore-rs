pragma solcore;

import * from std;

enum NoEq { NoEq }

// std.Generic is not imported.
#[derive(Eq)]
enum Wrap {
    Wrap(word)
}

contract K {
    function main() public returns (bool) {
        return Eq.eq(Wrap(1), Wrap(1));
    }
}
