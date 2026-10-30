pragma solcore;

import * from std;
import * from std.dispatch;
import { allA } from a;
import { allB } from b;
import { cycle } from cycle;

contract SuperclassDictionaries {
    // #[() -> 36]
    function run() public returns (uint256) {
        return uint256(primAddWord(allA(), allB()));
    }
    // #[(3) -> 1]
    function cyclic(n: uint256) public returns (uint256) {
        return uint256(cycle(Typedef.rep(n)));
    }
}
