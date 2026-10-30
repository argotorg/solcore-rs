pragma solcore;

import * from std;
import * from std.dispatch;
import { mload } from std.opcodes;

// As in Solidity, an oversized allocation panics with 0x41.
// #[scenario("memory-allocation-bounds")]
// #[deploy(0x) -> success]
// #[raw(0x90ca796b07ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, label = "array byte size wraps to zero") -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000041)]
// #[raw(0x90ca796b0800000000000000000000000000000000000000000000000000000000000000, label = "array byte size wraps to one word") -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000041)]
// #[raw(0x90ca796bffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, label = "array length plus header wraps") -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000041)]
// #[raw(0x7668bb4d, label = "array end pointer wraps") -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000041)]
// #[raw(0x75d838c30000000000000000000000000000000000000000000000000000000000000000, label = "empty array reserves its header") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x75d838c30000000000000000000000000000000000000000000000000000000000000003, label = "small array reserves header and elements") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
contract C {
    function allocate(length: uint256) public returns (uint256) {
        let values = allocateDynamicArray(@uint256, Typedef.rep(length));
        return uint256(mload(Typedef.rep(values)));
    }

    function pointerOverflow() public returns (uint256) {
        // The length itself fits, but advancing the current allocation pointer
        // must not wrap into already allocated memory.
        let values = allocateDynamicArray(@uint256, (~0 / 32) - 1);
        return uint256(mload(Typedef.rep(values)));
    }

    function small(length: uint256) public returns (uint256) {
        let before = get_free_memory();
        let values = allocateDynamicArray(@uint256, Typedef.rep(length));
        let after = get_free_memory();
        return (Typedef.rep(values) == before && mload(before) == Typedef.rep(length)
            && after - before == (Typedef.rep(length) + 1) * 32) ? uint256(1) : uint256(0);
    }
}
