pragma solcore;

import * from std;

// Control: only `t: Ord`, so `t: Eq` has a single source.
function same<t>(x: t, y: t) returns (bool) where t: Ord {
    return Eq.eq(x, y);
}

contract K {
    function main() public returns (bool) {
        let a: uint256 = 1;
        return same(a, a);
    }
}
