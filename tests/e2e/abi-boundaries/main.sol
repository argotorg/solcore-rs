pragma solcore;

import * from std;
import * from std.dispatch;

contract AbiBoundaries {
    // #[(0) -> 0]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    function echoUint(value: uint256) public returns (uint256) {
        return value;
    }

    // uint256 arithmetic is checked: overflow reverts with Panic(0x11).
    // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe, 1) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 1) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function checkedAdd(lhs: uint256, rhs: uint256) public returns (uint256) {
        return lhs + rhs;
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffff]
    function echoAddress(value: address) public returns (address) {
        return value;
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    function echoBytes32(value: bytes32) public returns (bytes32) {
        return value;
    }
}
