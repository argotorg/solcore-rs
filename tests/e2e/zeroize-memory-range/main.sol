pragma solcore;

import * from std;
import * from std.dispatch;

contract ZeroizeMemoryRange {
    // #[(0) -> 1]
    // #[(1) -> 1]
    // #[(31) -> 1]
    // #[(32) -> 1]
    // #[(33) -> 1]
    // #[(65) -> 1]
    function exactRange(length: uint256) public returns (uint256) {
        let length: word = Typedef.rep(length);
        let total: word = primAddWord(length, 64);
        let base: word = allocate_memory(total);
        let target: word = primAddWord(base, 31);
        let correct: word = 1;
        assembly {
            for {
                let i := 0
            } lt(i, total) {
                i := add(i, 1)
            } {
                mstore8(add(base, i), 0xa5)
            }
            // Clearing must not depend on the contents of a fixed zero slot.
            mstore(0x60, not(0))
        }
        zeroize_memory(target, length);
        assembly {
            mstore(0x60, 0)
            for {
                let i := 0
            } lt(i, total) {
                i := add(i, 1)
            } {
                let expected := 0xa5
                if and(iszero(lt(i, 31)), lt(i, add(31, length))) {
                    expected := 0
                }
                if iszero(eq(byte(0, mload(add(base, i))), expected)) {
                    correct := 0
                }
            }
        }
        return uint256(correct);
    }
}
