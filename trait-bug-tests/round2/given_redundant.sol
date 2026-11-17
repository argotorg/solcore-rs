pragma solcore;

import * from std;

// `t: Eq` is stated directly and is also a superclass of `t: Ord`.
function same<t>(x: t, y: t) returns (bool) where t: Eq, t: Ord {
    return Eq.eq(x, y);
}

contract K {
    function main() public returns (bool) {
        let a: uint256 = 1;
        return same(a, a);
    }
}
