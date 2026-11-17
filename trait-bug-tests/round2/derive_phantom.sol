pragma solcore;

import * from std;
import * from std.Generic;

enum NoEq { NoEq }

// `a` is phantom: comparing `Tag<a>` values never compares an `a`.
#[derive(Eq)]
enum Tag<a> {
    Tag(uint256)
}

contract K {
    function main() public returns (bool) {
        let x: Tag<NoEq> = Tag(1);
        return Eq.eq(x, x);
    }
}
