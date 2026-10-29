pragma solcore;

import * from std;
import * from std.dispatch;

// Storage-array primitives end to end: push / pop / length / indexed read,
// the two revert paths (index out of range, pop on empty), and the guarantee
// that abandoned slots are zeroed -- so regrowing an array never resurrects the
// values that `pop` or a shrinking `setLength` dropped.
// #[scenario("array_ops")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x56d88e27, label = "len() - initially 0") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x02899203, label = "popArr() on empty - reverts out_of_bounds()") -> revert(0xb4120f14)]
// #[raw(0x068f69ab000000000000000000000000000000000000000000000000000000000000000b, label = "pushVal(11)") -> success(0x)]
// #[raw(0x068f69ab0000000000000000000000000000000000000000000000000000000000000016, label = "pushVal(22)") -> success(0x)]
// #[raw(0x068f69ab0000000000000000000000000000000000000000000000000000000000000021, label = "pushVal(33)") -> success(0x)]
// #[raw(0x56d88e27, label = "len() - now 3") -> success(0x0000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000000, label = "get(0) - 11") -> success(0x000000000000000000000000000000000000000000000000000000000000000b)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000002, label = "get(2) - 33") -> success(0x0000000000000000000000000000000000000000000000000000000000000021)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000005, label = "get(5) out of range - reverts out_of_bounds()") -> revert(0xb4120f14)]
// #[raw(0x02899203, label = "popArr() - drops 33") -> success(0x)]
// #[raw(0x56d88e27, label = "len() - now 2") -> success(0x0000000000000000000000000000000000000000000000000000000000000002)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000001, label = "get(1) - still 22") -> success(0x0000000000000000000000000000000000000000000000000000000000000016)]
// #[raw(0x02899203, label = "popArr() - drops 22 and zeroes its slot") -> success(0x)]
// #[raw(0x28531f060000000000000000000000000000000000000000000000000000000000000002, label = "grow(2) - regrow to length 2") -> success(0x)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000001, label = "get(1) - 0, not the popped 22") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x28531f060000000000000000000000000000000000000000000000000000000000000000, label = "grow(0) - shrink to empty, clearing slots") -> success(0x)]
// #[raw(0x56d88e27, label = "len() - now 0") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x28531f060000000000000000000000000000000000000000000000000000000000000002, label = "grow(2) - regrow to length 2") -> success(0x)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000000, label = "get(0) - 0, not the cleared 11") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000001, label = "get(1) - 0") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
contract ArrayOps {
    xs: array<uint256>;

    constructor() {}

    // NOTE: not named `add` -- that collides with the Yul builtin of the same name.
    function pushVal(v: uint256) public {
        ArrayPush.push(xs, v);
    }

    function popArr() public {
        Array.pop(xs);
    }

    function len() public returns (uint256) {
        return Length.length(xs);
    }

    function get(i: uint256) public returns (uint256) {
        return xs[i];
    }

    function grow(n: uint256) public {
        Array.setLength(xs, n);
    }
}
