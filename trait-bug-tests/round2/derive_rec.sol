pragma solcore;

import * from std;
import * from std.Generic;

// A recursive type: its field `List<a>` needs the derived impl itself.
#[derive(Eq)]
enum List<a> {
    Nil,
    Cons(a, List<a>)
}

contract K {
    function main() public returns (bool) {
        let w: uint256 = 1;
        return Eq.eq(List.Cons(w, List.Nil), List.Cons(w, List.Nil));
    }
}
