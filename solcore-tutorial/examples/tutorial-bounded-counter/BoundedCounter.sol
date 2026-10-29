pragma solcore;

import * from std;
import * from std.dispatch;

contract BoundedCounter {
    count: uint256;

    // Invariant: count <= 10, established by zero-initialized storage.
    // Check the remaining capacity before adding, so neither operation wraps.
    // Every accepted update preserves the invariant.
    // #[send(4)]
    // #[send(6)]
    // #[(1) -> revert]
    function add(amount: uint256) public {
        if (amount > uint256(10) - count) {
            revertWithError("capacity exceeded");
        }
        count = count + amount;
    }

    // #[() -> 10]
    function read() public returns (uint256) {
        return count;
    }

    // #[() -> 0]
    function remaining() public returns (uint256) {
        return uint256(10) - count;
    }
}
