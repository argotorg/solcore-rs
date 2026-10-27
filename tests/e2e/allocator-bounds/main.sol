pragma solcore;

import * from std;
import * from std.dispatch;

contract AllocatorBounds {
    // As in Solidity, an allocation that wraps the free pointer panics with 0x41.
    // #[(4096, 0) -> 0]
    // #[(4096, 1) -> 1]
    // #[(4096, 32) -> 32]
    // #[(4096, 33) -> 33]
    // #[(115792089237316195423570985008687907853269984665640564039457584007913129639920, 32) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000041)]
    // #[(4096, 115792089237316195423570985008687907853269984665640564039457584007913129639935) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000041)]
    function allocatedSize(cursor: uint256, size: uint256) public returns (uint256) {
        let original = get_free_memory();
        set_free_memory(Typedef.rep(cursor));
        let start = allocate_memory(Typedef.rep(size));
        let end = get_free_memory();
        set_free_memory(original);
        return uint256(end - start);
    }
}
