pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;
import { mload, mstore } from std.opcodes;

enum StaticChoice {
    Empty,
    Small(uint256),
    Wide(uint256, uint256)
}

enum DynamicChoice {
    Empty,
    Small(uint256),
    Wide(memory<bytes>, uint256)
}

contract C {
    // Encoding into previously used scratch memory must initialize the entire
    // reserved head, including words absent from the selected variant.
    // #[() -> 0]
    function primitiveLeft() public returns (uint256) {
        let target = get_free_memory();
        mstore(target + 64, 0xdeadbeef);
        let value: sum<(), uint256> = inl(());
        let encoded = abi_encode(value);
        return uint256(mload(Typedef.rep(encoded) + 64));
    }

    // #[() -> 0]
    function primitiveRight() public returns (uint256) {
        let target = get_free_memory();
        mstore(target + 64, 0xdeadbeef);
        let value: sum<uint256, ()> = inr(());
        let encoded = abi_encode(value);
        return uint256(mload(Typedef.rep(encoded) + 64));
    }

    // #[() -> 0]
    function derivedEmpty() public returns (uint256) {
        let target = get_free_memory();
        mstore(target + 64, 0xdeadbeef);
        mstore(target + 96, 0xcafebabe);
        let encoded = abi_encode(StaticChoice.Empty);
        let ptr = Typedef.rep(encoded);
        return uint256(mload(ptr + 64) | mload(ptr + 96));
    }

    // #[() -> 42]
    function derivedSmall() public returns (uint256) {
        let target = get_free_memory();
        mstore(target + 96, 0xdeadbeef);
        let encoded = abi_encode(StaticChoice.Small(42));
        let ptr = Typedef.rep(encoded);
        return uint256(mload(ptr + 64) + mload(ptr + 96));
    }

    // #[() -> 0]
    function dynamicEmpty() public returns (uint256) {
        let target = get_free_memory();
        mstore(target + 96, 0xdeadbeef);
        mstore(target + 128, 0xcafebabe);
        let encoded = abi_encode(DynamicChoice.Empty);
        let ptr = Typedef.rep(encoded);
        return uint256(mload(ptr + 96) | mload(ptr + 128));
    }

    // #[() -> 42]
    function dynamicSmall() public returns (uint256) {
        let target = get_free_memory();
        mstore(target + 128, 0xdeadbeef);
        let encoded = abi_encode(DynamicChoice.Small(42));
        let ptr = Typedef.rep(encoded);
        return uint256(mload(ptr + 96) + mload(ptr + 128));
    }

    // Primitive dynamic sums share the same padding obligation.
    // #[() -> 0]
    function dynamicPrimitive() public returns (uint256) {
        let target = get_free_memory();
        mstore(target + 96, 0xdeadbeef);
        let value: sum<(), memory<bytes>> = inl(());
        let encoded = abi_encode(value);
        return uint256(mload(Typedef.rep(encoded) + 96));
    }

    // Padding is local to this sum: preserve the following tuple field.
    // #[() -> 7]
    function tupleSibling() public returns (uint256) {
        let target = get_free_memory();
        mstore(target + 64, 0xdeadbeef);
        mstore(target + 96, 0xcafebabe);
        let encoded = abi_encode((StaticChoice.Empty, uint256(7)));
        let ptr = Typedef.rep(encoded);
        return uint256(mload(ptr + 64) + mload(ptr + 96) + mload(ptr + 128));
    }
}
